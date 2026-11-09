//! In-process JavaScript virtual machine.
//!
//! Boa runs ECMAScript in this process, including modules. Host objects are the
//! window, document, and node tree. Timers and `location` report back to the browser.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use boa_engine::module::{Module, ModuleLoader, ModuleRequest, Referrer};
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, Finalize, JsData, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    Source, Trace, js_string,
};
use mithu_css::{parse_css, parse_declarations, Declaration};
use mithu_dom::{Document, NodeData, NodeId};
use mithu_html::parse_html;
use mithu_style::matches_element;

const ATTR: Attribute = Attribute::WRITABLE.union(Attribute::ENUMERABLE).union(Attribute::CONFIGURABLE);

#[derive(Clone)]
struct Host {
    doc: Rc<RefCell<Document>>,
    log: Rc<RefCell<Vec<String>>>,
    prevented: Rc<Cell<bool>>,
    location: Rc<RefCell<String>>,
    navigation: Rc<RefCell<Option<String>>>,
    clock: Rc<RefCell<Clock>>,
    event_target: Rc<Cell<Option<usize>>>,
    event_current: Rc<Cell<Option<usize>>>,
}

struct Clock {
    now: u64,
    next: u32,
    queue: Vec<Timer>,
}

struct Timer {
    id: u32,
    due: u64,
    repeat: Option<u64>,
}

struct PageLoader {
    fetch: RefCell<Box<dyn FnMut(&str) -> Option<String>>>,
    cache: RefCell<HashMap<String, Module>>,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct NodeKey {
    #[unsafe_ignore_trace]
    id: usize,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct ResponseKey {
    #[unsafe_ignore_trace]
    status: u16,
    #[unsafe_ignore_trace]
    ok: bool,
    #[unsafe_ignore_trace]
    url: String,
    #[unsafe_ignore_trace]
    body: String,
}

pub fn engine_name() -> &'static str {
    "boa"
}

pub struct Session {
    context: Context,
    doc: Rc<RefCell<Document>>,
    log: Rc<RefCell<Vec<String>>>,
    prevented: Rc<Cell<bool>>,
}

impl Session {
    pub fn open(doc: Document, fetch: impl FnMut(&str) -> Option<String> + 'static) -> Self {
        Self::open_at(doc, "about:blank", fetch)
    }

    pub fn open_at(doc: Document, href: &str, fetch: impl FnMut(&str) -> Option<String> + 'static) -> Self {
        let log = Rc::new(RefCell::new(Vec::new()));
        let prevented = Rc::new(Cell::new(false));
        let shared = Rc::new(RefCell::new(doc));
        let location = Rc::new(RefCell::new(href.to_string()));
        let navigation = Rc::new(RefCell::new(None));
        let clock = Rc::new(RefCell::new(Clock { now: 0, next: 0, queue: Vec::new() }));
        let loader = Rc::new(PageLoader {
            fetch: RefCell::new(Box::new(fetch)),
            cache: RefCell::new(HashMap::new()),
        });
        let mut context = Context::builder()
            .module_loader(Rc::clone(&loader))
            .build()
            .unwrap_or_else(|_| Context::default());
        context.insert_data(Host {
            doc: Rc::clone(&shared),
            log: Rc::clone(&log),
            prevented: Rc::clone(&prevented),
            location,
            navigation,
            clock,
            event_target: Rc::new(Cell::new(None)),
            event_current: Rc::new(Cell::new(None)),
        });
        if let Err(error) = install_host(&mut context) {
            log.borrow_mut().push(format!("JavaScript error: {error}"));
        } else {
            let scripts = shared.borrow().scripts.clone();
            for script in scripts {
                let code = if let Some(src) = &script.src {
                    let fetched = {
                        let mut fetch = loader.fetch.borrow_mut();
                        fetch(src).unwrap_or_default()
                    };
                    fetched
                } else {
                    script.code.clone()
                };
                if code.trim().is_empty() {
                    continue;
                }
                let result = if script.module {
                    run_module(&mut context, &code, script.src.as_deref().unwrap_or("inline-module.js"))
                } else if let Err(error) = context.eval(Source::from_bytes(code.as_str())) {
                    Err(error)
                } else {
                    context.run_jobs()
                };
                if let Err(error) = result {
                    log.borrow_mut().push(format!("JavaScript error: {error}"));
                }
            }
            if let Err(error) = run_due_timers(&mut context, 0) {
                log.borrow_mut().push(format!("JavaScript error: {error}"));
            }
        }
        Self {
            context,
            doc: shared,
            log,
            prevented,
        }
    }

    pub fn document(&self) -> Document {
        self.doc.borrow().clone()
    }

    pub fn edit(&mut self, update: impl FnOnce(&mut Document)) {
        update(&mut self.doc.borrow_mut());
    }

    pub fn logs(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    pub fn take_navigation(&mut self) -> Option<String> {
        self.context.get_data::<Host>().cloned().and_then(|host| host.navigation.borrow_mut().take())
    }

    /// Moves the page clock forward and runs timers whose delay has elapsed.
    pub fn advance(&mut self, ms: u64) -> bool {
        match run_due_timers(&mut self.context, ms) {
            Ok(ran) => ran,
            Err(error) => {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                false
            }
        }
    }

    pub fn next_timer_ms(&self) -> Option<u64> {
        let host = self.context.get_data::<Host>().cloned()?;
        let clock = host.clock.borrow();
        clock.queue.iter().map(|timer| timer.due.saturating_sub(clock.now)).min()
    }

    /// Runs listeners from the target up through its ancestors.
    /// Returns true when the browser should still perform the default action.
    pub fn dispatch(&mut self, id: NodeId, event: &str) -> bool {
        self.prevented.set(false);
        let chain = {
            let doc = self.doc.borrow();
            let start = if doc.element_name(id).is_some() {
                Some(id)
            } else {
                doc.parent_element(id)
            };
            let mut chain = Vec::new();
            let mut current = start;
            while let Some(node) = current {
                chain.push(node);
                current = doc.parent_element(node);
            }
            chain
        };
        for node in chain {
            self.invoke(node, id, event);
        }
        self.invoke_window(id, event);
        !self.prevented.get()
    }

    pub fn submission(&mut self, control: NodeId) -> Option<Submission> {
        let doc = self.document();
        let form = form_owner(&doc, control)?;
        if !self.dispatch(form, "submit") {
            return None;
        }
        let doc = self.document();
        let action = doc.attr(form, "action").unwrap_or("").to_string();
        let method = doc.attr(form, "method").unwrap_or("get").to_ascii_lowercase();
        let method = if method == "post" { "post" } else { "get" };
        Some(Submission {
            action,
            method: method.to_string(),
            query: form_query(&doc, form),
        })
    }

    fn invoke(&mut self, current: NodeId, target: NodeId, event: &str) {
        self.ensure_attribute_handler(current, event);
        let functions = match listener_functions(&mut self.context, current, event) {
            Ok(functions) => functions,
            Err(error) => {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                return;
            }
        };
        if functions.is_empty() {
            return;
        }
        if let Some(host) = self.context.get_data::<Host>().cloned() {
            host.event_target.set(Some(target.0));
            host.event_current.set(Some(current.0));
        }
        let this = element_value(current.0, &mut self.context);
        let event_value = match event_object(target, event, &mut self.context) {
            Ok(value) => value,
            Err(error) => {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                return;
            }
        };
        for function in functions {
            if let Err(error) = function.call(&this, &[event_value.clone()], &mut self.context) {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
            }
        }
    }

    fn invoke_window(&mut self, target: NodeId, event: &str) {
        if let Some(host) = self.context.get_data::<Host>().cloned() {
            host.event_target.set(Some(target.0));
            host.event_current.set(None);
        }
        let functions = match read_listeners(&mut self.context, &format!("win:{event}")) {
            Ok(functions) => functions,
            Err(error) => {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                return;
            }
        };
        if functions.is_empty() {
            return;
        }
        let event_value = match event_object(target, event, &mut self.context) {
            Ok(value) => value,
            Err(error) => {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                return;
            }
        };
        let this = JsValue::from(self.context.global_object().clone());
        for function in functions {
            if let Err(error) = function.call(&this, &[event_value.clone()], &mut self.context) {
                self.log.borrow_mut().push(format!("JavaScript error: {error}"));
            }
        }
    }

    fn ensure_attribute_handler(&mut self, id: NodeId, event: &str) {
        let code = {
            let doc = self.doc.borrow();
            doc.attr(id, &format!("on{event}")).unwrap_or("").to_string()
        };
        if code.trim().is_empty() {
            return;
        }
        let key = format!("on:{}:{event}", id.0);
        if listener_count(&mut self.context, &key).unwrap_or(0) > 0 {
            return;
        }
        let source = format!("(function(event) {{\n{code}\n}})");
        match self.context.eval(Source::from_bytes(source.as_str())) {
            Ok(value) => {
                if let Err(error) = push_listener(&mut self.context, &key, &value, false) {
                    self.log.borrow_mut().push(format!("JavaScript error: {error}"));
                }
            }
            Err(error) => self.log.borrow_mut().push(format!("JavaScript error: {error}")),
        }
    }
}

pub struct Submission {
    pub action: String,
    pub method: String,
    pub query: String,
}

pub fn run_scripts(doc: &mut Document, fetch: impl FnMut(&str) -> Option<String> + 'static) -> Vec<String> {
    let session = Session::open(std::mem::take(doc), fetch);
    let logs = session.logs();
    *doc = session.document();
    logs
}

fn install_host(context: &mut Context) -> JsResult<()> {
    let listeners = ObjectInitializer::new(context).build();
    let document = document_object(context)?;
    let console = console_object(context)?;
    let flags = ATTR;
    let timers = ObjectInitializer::new(context).build();
    let window = context.global_object().clone();
    context.register_global_property(js_string!("__mithuListeners"), listeners, flags)?;
    context.register_global_property(js_string!("__mithuTimers"), timers, flags)?;
    context.register_global_property(js_string!("document"), document, flags)?;
    context.register_global_property(js_string!("console"), console, flags)?;
    let navigator = navigator_object(context)?;
    let location = location_object(context)?;
    context.register_global_property(js_string!("window"), window, flags)?;
    context.register_global_property(js_string!("navigator"), navigator, flags)?;
    context.register_global_property(js_string!("location"), location, flags)?;
    context.register_global_callable(js_string!("setTimeout"), 2, NativeFunction::from_fn_ptr(set_timeout))?;
    context.register_global_callable(js_string!("setInterval"), 2, NativeFunction::from_fn_ptr(set_interval))?;
    context.register_global_callable(js_string!("clearTimeout"), 1, NativeFunction::from_fn_ptr(clear_timer))?;
    context.register_global_callable(js_string!("clearInterval"), 1, NativeFunction::from_fn_ptr(clear_timer))?;
    context.register_global_callable(js_string!("addEventListener"), 2, NativeFunction::from_fn_ptr(window_listener))?;
    context.register_global_callable(js_string!("fetch"), 1, NativeFunction::from_fn_ptr(fetch_resource))?;
    Ok(())
}

fn console_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object.function(NativeFunction::from_fn_ptr(console_log), js_string!("log"), 1);
    object.function(NativeFunction::from_fn_ptr(console_log), js_string!("warn"), 1);
    object.function(NativeFunction::from_fn_ptr(console_log), js_string!("error"), 1);
    Ok(object.build())
}

fn navigator_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object.property(js_string!("userAgent"), js_string!("Mithu"), ATTR);
    Ok(object.build())
}

fn location_object(context: &mut Context) -> JsResult<JsObject> {
    let href_get = native(context, "get href", 0, get_location);
    let href_set = native(context, "set href", 1, set_location);
    let mut object = ObjectInitializer::new(context);
    object.accessor(js_string!("href"), Some(href_get), Some(href_set), ATTR);
    Ok(object.build())
}

fn document_object(context: &mut Context) -> JsResult<JsObject> {
    let title_get = native(context, "get title", 0, get_title);
    let title_set = native(context, "set title", 1, set_title);
    let body_get = native(context, "get body", 0, get_body);
    let document_get = native(context, "get documentElement", 0, get_document_element);
    let head_get = native(context, "get head", 0, get_head);
    let mut object = ObjectInitializer::new(context);
    object
        .function(NativeFunction::from_fn_ptr(get_element_by_id), js_string!("getElementById"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector), js_string!("querySelector"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector_all), js_string!("querySelectorAll"), 1)
        .function(NativeFunction::from_fn_ptr(create_element), js_string!("createElement"), 1)
        .function(NativeFunction::from_fn_ptr(create_text_node), js_string!("createTextNode"), 1)
        .function(NativeFunction::from_fn_ptr(get_elements_by_tag_name), js_string!("getElementsByTagName"), 1)
        .function(NativeFunction::from_fn_ptr(get_elements_by_class_name), js_string!("getElementsByClassName"), 1)
        .function(NativeFunction::from_fn_ptr(document_listener), js_string!("addEventListener"), 2)
        .accessor(js_string!("title"), Some(title_get), Some(title_set), ATTR)
        .accessor(js_string!("body"), Some(body_get), None, ATTR)
        .accessor(js_string!("documentElement"), Some(document_get), None, ATTR)
        .accessor(js_string!("head"), Some(head_get), None, ATTR);
    Ok(object.build())
}

fn native(
    context: &mut Context,
    name: &str,
    length: usize,
    body: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
) -> boa_engine::object::builtins::JsFunction {
    FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(body))
        .name(JsString::from(name))
        .length(length)
        .constructor(false)
        .build()
}

fn console_log(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let mut parts = Vec::new();
    for arg in args {
        parts.push(arg.to_string(context)?.to_std_string_lossy());
    }
    host.log.borrow_mut().push(parts.join(" "));
    Ok(JsValue::undefined())
}

fn get_title(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(js_string!(host.doc.borrow().title.clone()).into())
}

fn set_title(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let value = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    doc.title = value.clone();
    if let Some(title) = find_tag(&doc, "title") {
        doc.set_text_content(title, &value);
    }
    Ok(JsValue::undefined())
}

fn get_body(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = find_tag(&host.doc.borrow(), "body");
    match id {
        Some(id) => Ok(element_value(id.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn get_element_by_id(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = arg_string(args, 0, context)?;
    let found = host.doc.borrow().get_element_by_id(&id);
    match found {
        Some(node) => Ok(element_value(node.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn query_selector(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let scope = node_of(this).unwrap_or(NodeId(0));
    let found = {
        let doc = host.doc.borrow();
        query_first(&doc, scope, &selector, node_of(this).is_some())
    };
    match found {
        Some(node) => Ok(element_value(node.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn query_selector_all(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let scope = node_of(this).unwrap_or(NodeId(0));
    let ids = {
        let doc = host.doc.borrow();
        query_all(&doc, scope, &selector, node_of(this).is_some())
    };
    let values = ids.into_iter().map(|id| element_value(id.0, context)).collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn get_elements_by_tag_name(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?.to_ascii_lowercase();
    collect_named(this, context, |doc, id| {
        doc.element_name(id).is_some_and(|element| name == "*" || element == name)
    })
}

fn get_elements_by_class_name(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let class = arg_string(args, 0, context)?;
    collect_named(this, context, move |doc, id| doc.has_class(id, &class))
}

fn collect_named(
    this: &JsValue,
    context: &mut Context,
    mut keep: impl FnMut(&Document, NodeId) -> bool,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let ids = {
        let doc = host.doc.borrow();
        let mut ids = Vec::new();
        if let Some(scope) = node_of(this) {
            for child in doc.node(scope).children.clone() {
                collect_matching(&doc, child, &mut keep, &mut ids);
            }
        } else {
            collect_matching(&doc, doc.root, &mut keep, &mut ids);
        }
        ids
    };
    let values = ids.into_iter().map(|id| element_value(id.0, context)).collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn collect_matching(doc: &Document, id: NodeId, keep: &mut impl FnMut(&Document, NodeId) -> bool, out: &mut Vec<NodeId>) {
    if keep(doc, id) {
        out.push(id);
    }
    for child in doc.node(id).children.clone() {
        collect_matching(doc, child, keep, out);
    }
}

fn create_element(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let name = arg_string(args, 0, context)?;
    let id = host.doc.borrow_mut().create_element_detached(&name);
    Ok(element_value(id.0, context))
}

fn element_value(id: usize, context: &mut Context) -> JsValue {
    let text_get = native(context, "get textContent", 0, get_text);
    let text_set = native(context, "set textContent", 1, set_text);
    let inner_get = native(context, "get innerText", 0, get_text);
    let inner_set = native(context, "set innerText", 1, set_text);
    let html_get = native(context, "get innerHTML", 0, get_html);
    let html_set = native(context, "set innerHTML", 1, set_html);
    let id_get = native(context, "get id", 0, get_id);
    let id_set = native(context, "set id", 1, set_id);
    let class_get = native(context, "get className", 0, get_class);
    let class_set = native(context, "set className", 1, set_class);
    let click_set = native(context, "set onclick", 1, set_onclick);
    let input_set = native(context, "set oninput", 1, set_oninput);
    let change_set = native(context, "set onchange", 1, set_onchange);
    let submit_set = native(context, "set onsubmit", 1, set_onsubmit);
    let style = style_object(id, context);
    let class_list = class_list_object(id, context);
    let parent_get = native(context, "get parentElement", 0, get_parent);
    let next_get = native(context, "get nextElementSibling", 0, get_next_sibling);
    let previous_get = native(context, "get previousElementSibling", 0, get_previous_sibling);
    let children_get = native(context, "get children", 0, get_children);
    let nodes_get = native(context, "get childNodes", 0, get_child_nodes);
    let first_get = native(context, "get firstChild", 0, get_first_child);
    let last_get = native(context, "get lastChild", 0, get_last_child);
    let next_node_get = native(context, "get nextSibling", 0, get_next_node);
    let previous_node_get = native(context, "get previousSibling", 0, get_previous_node);
    let parent_node_get = native(context, "get parentNode", 0, get_parent_node);
    let type_get = native(context, "get nodeType", 0, get_node_type);
    let name_get = native(context, "get nodeName", 0, get_node_name);
    let value_get = native(context, "get value", 0, get_value);
    let value_set = native(context, "set value", 1, set_value);
    let checked_get = native(context, "get checked", 0, get_checked);
    let checked_set = native(context, "set checked", 1, set_checked);
    let href_get = native(context, "get href", 0, get_href);
    let href_set = native(context, "set href", 1, set_href);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .function(NativeFunction::from_fn_ptr(get_attribute), js_string!("getAttribute"), 1)
        .function(NativeFunction::from_fn_ptr(set_attribute), js_string!("setAttribute"), 2)
        .function(NativeFunction::from_fn_ptr(remove_attribute), js_string!("removeAttribute"), 1)
        .function(NativeFunction::from_fn_ptr(has_attribute), js_string!("hasAttribute"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector), js_string!("querySelector"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector_all), js_string!("querySelectorAll"), 1)
        .function(NativeFunction::from_fn_ptr(get_elements_by_tag_name), js_string!("getElementsByTagName"), 1)
        .function(NativeFunction::from_fn_ptr(get_elements_by_class_name), js_string!("getElementsByClassName"), 1)
        .function(NativeFunction::from_fn_ptr(matches_selector), js_string!("matches"), 1)
        .function(NativeFunction::from_fn_ptr(closest_selector), js_string!("closest"), 1)
        .function(NativeFunction::from_fn_ptr(contains_node), js_string!("contains"), 1)
        .function(NativeFunction::from_fn_ptr(dispatch_event), js_string!("dispatchEvent"), 1)
        .function(NativeFunction::from_fn_ptr(append_child), js_string!("appendChild"), 1)
        .function(NativeFunction::from_fn_ptr(insert_before_child), js_string!("insertBefore"), 2)
        .function(NativeFunction::from_fn_ptr(remove_child), js_string!("removeChild"), 1)
        .function(NativeFunction::from_fn_ptr(add_event_listener), js_string!("addEventListener"), 2)
        .function(NativeFunction::from_fn_ptr(remove_event_listener), js_string!("removeEventListener"), 2)
        .accessor(js_string!("onclick"), None, Some(click_set), ATTR)
        .accessor(js_string!("oninput"), None, Some(input_set), ATTR)
        .accessor(js_string!("onchange"), None, Some(change_set), ATTR)
        .accessor(js_string!("onsubmit"), None, Some(submit_set), ATTR)
        .accessor(js_string!("textContent"), Some(text_get), Some(text_set), ATTR)
        .accessor(js_string!("innerText"), Some(inner_get), Some(inner_set), ATTR)
        .accessor(js_string!("innerHTML"), Some(html_get), Some(html_set), ATTR)
        .accessor(js_string!("id"), Some(id_get), Some(id_set), ATTR)
        .accessor(js_string!("className"), Some(class_get), Some(class_set), ATTR)
        .accessor(js_string!("parentElement"), Some(parent_get), None, ATTR)
        .accessor(js_string!("nextElementSibling"), Some(next_get), None, ATTR)
        .accessor(js_string!("previousElementSibling"), Some(previous_get), None, ATTR)
        .accessor(js_string!("children"), Some(children_get), None, ATTR)
        .accessor(js_string!("childNodes"), Some(nodes_get), None, ATTR)
        .accessor(js_string!("firstChild"), Some(first_get), None, ATTR)
        .accessor(js_string!("lastChild"), Some(last_get), None, ATTR)
        .accessor(js_string!("nextSibling"), Some(next_node_get), None, ATTR)
        .accessor(js_string!("previousSibling"), Some(previous_node_get), None, ATTR)
        .accessor(js_string!("parentNode"), Some(parent_node_get), None, ATTR)
        .accessor(js_string!("nodeType"), Some(type_get), None, ATTR)
        .accessor(js_string!("nodeName"), Some(name_get.clone()), None, ATTR)
        .accessor(js_string!("tagName"), Some(name_get), None, ATTR)
        .accessor(js_string!("value"), Some(value_get), Some(value_set), ATTR)
        .accessor(js_string!("checked"), Some(checked_get), Some(checked_set), ATTR)
        .accessor(js_string!("href"), Some(href_get), Some(href_set), ATTR)
        .property(js_string!("classList"), class_list, ATTR)
        .property(js_string!("style"), style, ATTR);
    object.build().into()
}

fn style_object(id: usize, context: &mut Context) -> JsObject {
    let css_get = native(context, "get cssText", 0, get_css_text);
    let css_set = native(context, "set cssText", 1, set_css_text);
    let color_get = native(context, "get color", 0, get_style_color);
    let color_set = native(context, "set color", 1, set_style_color);
    let background_get = native(context, "get backgroundColor", 0, get_style_background);
    let background_set = native(context, "set backgroundColor", 1, set_style_background);
    let display_get = native(context, "get display", 0, get_style_display);
    let display_set = native(context, "set display", 1, set_style_display);
    let width_get = native(context, "get width", 0, get_style_width);
    let width_set = native(context, "set width", 1, set_style_width);
    let height_get = native(context, "get height", 0, get_style_height);
    let height_set = native(context, "set height", 1, set_style_height);
    let font_get = native(context, "get fontSize", 0, get_style_font_size);
    let font_set = native(context, "set fontSize", 1, set_style_font_size);
    let margin_get = native(context, "get margin", 0, get_style_margin);
    let margin_set = native(context, "set margin", 1, set_style_margin);
    let padding_get = native(context, "get padding", 0, get_style_padding);
    let padding_set = native(context, "set padding", 1, set_style_padding);
    let align_get = native(context, "get textAlign", 0, get_style_align);
    let align_set = native(context, "set textAlign", 1, set_style_align);
    let position_get = native(context, "get position", 0, get_style_position);
    let position_set = native(context, "set position", 1, set_style_position);
    let top_get = native(context, "get top", 0, get_style_top);
    let top_set = native(context, "set top", 1, set_style_top);
    let left_get = native(context, "get left", 0, get_style_left);
    let left_set = native(context, "set left", 1, set_style_left);
    let opacity_get = native(context, "get opacity", 0, get_style_opacity);
    let opacity_set = native(context, "set opacity", 1, set_style_opacity);
    let flex_get = native(context, "get flexDirection", 0, get_style_flex_direction);
    let flex_set = native(context, "set flexDirection", 1, set_style_flex_direction);
    let gap_get = native(context, "get gap", 0, get_style_gap);
    let gap_set = native(context, "set gap", 1, set_style_gap);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("cssText"), Some(css_get), Some(css_set), ATTR)
        .function(NativeFunction::from_fn_ptr(set_property), js_string!("setProperty"), 2)
        .accessor(js_string!("color"), Some(color_get), Some(color_set), ATTR)
        .accessor(js_string!("backgroundColor"), Some(background_get), Some(background_set), ATTR)
        .accessor(js_string!("display"), Some(display_get), Some(display_set), ATTR)
        .accessor(js_string!("width"), Some(width_get), Some(width_set), ATTR)
        .accessor(js_string!("height"), Some(height_get), Some(height_set), ATTR)
        .accessor(js_string!("fontSize"), Some(font_get), Some(font_set), ATTR)
        .accessor(js_string!("margin"), Some(margin_get), Some(margin_set), ATTR)
        .accessor(js_string!("padding"), Some(padding_get), Some(padding_set), ATTR)
        .accessor(js_string!("textAlign"), Some(align_get), Some(align_set), ATTR)
        .accessor(js_string!("position"), Some(position_get), Some(position_set), ATTR)
        .accessor(js_string!("top"), Some(top_get), Some(top_set), ATTR)
        .accessor(js_string!("left"), Some(left_get), Some(left_set), ATTR)
        .accessor(js_string!("opacity"), Some(opacity_get), Some(opacity_set), ATTR)
        .accessor(js_string!("flexDirection"), Some(flex_get), Some(flex_set), ATTR)
        .accessor(js_string!("gap"), Some(gap_get), Some(gap_set), ATTR);
    object.build()
}

macro_rules! style_fns {
    ($get:ident, $set:ident, $css:literal) => {
        fn $get(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            read_style(this, $css, context)
        }
        fn $set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            write_style(this, $css, args, context)
        }
    };
}

style_fns!(get_style_color, set_style_color, "color");
style_fns!(get_style_background, set_style_background, "background-color");
style_fns!(get_style_display, set_style_display, "display");
style_fns!(get_style_width, set_style_width, "width");
style_fns!(get_style_height, set_style_height, "height");
style_fns!(get_style_font_size, set_style_font_size, "font-size");
style_fns!(get_style_margin, set_style_margin, "margin");
style_fns!(get_style_padding, set_style_padding, "padding");
style_fns!(get_style_align, set_style_align, "text-align");
style_fns!(get_style_position, set_style_position, "position");
style_fns!(get_style_top, set_style_top, "top");
style_fns!(get_style_left, set_style_left, "left");
style_fns!(get_style_opacity, set_style_opacity, "opacity");
style_fns!(get_style_flex_direction, set_style_flex_direction, "flex-direction");
style_fns!(get_style_gap, set_style_gap, "gap");

fn get_css_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let text = host.doc.borrow().attr(id, "style").unwrap_or("").to_string();
    Ok(js_string!(text).into())
}

fn set_css_text(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "style", &value);
    Ok(JsValue::undefined())
}

fn set_property(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    write_style(this, &name, &args[1..], context)
}

fn read_style(this: &JsValue, name: &str, context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let text = host.doc.borrow().attr(id, "style").unwrap_or("").to_string();
    let value = parse_declarations(&text)
        .into_iter()
        .rev()
        .find(|declaration| declaration.name == name)
        .map(|declaration| declaration.value)
        .unwrap_or_default();
    Ok(js_string!(value).into())
}

fn write_style(this: &JsValue, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    let current = doc.attr(id, "style").unwrap_or("").to_string();
    let mut declarations = parse_declarations(&current);
    if let Some(existing) = declarations.iter_mut().find(|declaration| declaration.name == name) {
        existing.value = value;
        existing.important = false;
    } else {
        declarations.push(Declaration {
            name: name.to_string(),
            value,
            important: false,
        });
    }
    let text = declarations
        .iter()
        .map(|declaration| format!("{}: {}", declaration.name, declaration.value))
        .collect::<Vec<_>>()
        .join("; ");
    doc.set_attr(id, "style", &text);
    Ok(JsValue::undefined())
}

fn get_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().text_content(id)).into())
}

fn set_text(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_text_content(id, &value);
    Ok(JsValue::undefined())
}

fn get_html(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(serialize(&host.doc.borrow(), id)).into())
}

fn set_html(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let html = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    doc.clear_children(id);
    if html.contains('<') {
        let parsed = parse_html(&format!("<div id=\"mithu-root\">{html}</div>"));
        if let Some(root) = parsed.get_element_by_id("mithu-root") {
            copy_children(&parsed, root, &mut doc, id);
        }
    } else if !html.is_empty() {
        doc.append_text(id, &html);
    }
    Ok(JsValue::undefined())
}

fn get_id(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, "id").unwrap_or("").to_string()).into())
}

fn set_id(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "id", &value);
    Ok(JsValue::undefined())
}

fn get_class(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, "class").unwrap_or("").to_string()).into())
}

fn set_class(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "class", &value);
    Ok(JsValue::undefined())
}

fn get_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    match host.doc.borrow().attr(id, &name) {
        Some(value) => Ok(js_string!(value.to_string()).into()),
        None => Ok(JsValue::null()),
    }
}

fn set_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    host.doc.borrow_mut().set_attr(id, &name, &value);
    Ok(JsValue::undefined())
}

fn class_list_object(id: usize, context: &mut Context) -> JsObject {
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .function(NativeFunction::from_fn_ptr(class_list_add), js_string!("add"), 1)
        .function(NativeFunction::from_fn_ptr(class_list_remove), js_string!("remove"), 1)
        .function(NativeFunction::from_fn_ptr(class_list_toggle), js_string!("toggle"), 1)
        .function(NativeFunction::from_fn_ptr(class_list_contains), js_string!("contains"), 1);
    object.build()
}

fn class_tokens(doc: &mithu_dom::Document, id: NodeId) -> Vec<String> {
    doc.attr(id, "class")
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn write_classes(doc: &mut mithu_dom::Document, id: NodeId, classes: &[String]) {
    doc.set_attr(id, "class", &classes.join(" "));
}

fn class_list_add(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let mut classes = class_tokens(&host.doc.borrow(), id);
    for arg in args {
        let token = arg.to_string(context)?.to_std_string_lossy();
        if !token.is_empty() && !classes.iter().any(|item| item == &token) {
            classes.push(token);
        }
    }
    write_classes(&mut host.doc.borrow_mut(), id, &classes);
    Ok(JsValue::undefined())
}

fn class_list_remove(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let mut classes = class_tokens(&host.doc.borrow(), id);
    for arg in args {
        let token = arg.to_string(context)?.to_std_string_lossy();
        classes.retain(|item| item != &token);
    }
    write_classes(&mut host.doc.borrow_mut(), id, &classes);
    Ok(JsValue::undefined())
}

fn class_list_toggle(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let token = arg_string(args, 0, context)?;
    let mut classes = class_tokens(&host.doc.borrow(), id);
    let present = classes.iter().any(|item| item == &token);
    if present {
        classes.retain(|item| item != &token);
    } else if !token.is_empty() {
        classes.push(token);
    }
    write_classes(&mut host.doc.borrow_mut(), id, &classes);
    Ok(JsValue::from(!present))
}

fn class_list_contains(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let token = arg_string(args, 0, context)?;
    Ok(JsValue::from(host.doc.borrow().has_class(id, &token)))
}

fn sibling_value(this: &JsValue, context: &mut Context, next: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let sibling = {
        let doc = host.doc.borrow();
        if next { doc.next_element_sibling(id) } else { doc.previous_element_sibling(id) }
    };
    match sibling {
        Some(node) => Ok(element_value(node.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn get_parent(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    match host.doc.borrow().parent_element(id) {
        Some(parent) => Ok(element_value(parent.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn get_next_sibling(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    sibling_value(this, context, true)
}

fn get_previous_sibling(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    sibling_value(this, context, false)
}

fn get_children(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let ids = {
        let doc = host.doc.borrow();
        doc.node(id)
            .children
            .iter()
            .copied()
            .filter(|child| doc.element_name(*child).is_some())
            .collect::<Vec<_>>()
    };
    let values = ids.into_iter().map(|child| element_value(child.0, context)).collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn remove_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    let belongs = host.doc.borrow().parent_element(child) == Some(parent);
    if !belongs {
        return Err(type_error());
    }
    host.doc.borrow_mut().detach(child);
    Ok(child_value)
}

fn append_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    host.doc.borrow_mut().append_child(parent, child);
    Ok(child_value)
}

fn add_event_listener(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    push_listener(context, &format!("add:{}:{event}", id.0), &handler, false)?;
    Ok(JsValue::undefined())
}

fn set_onclick(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    set_handler(this, "click", args, context)
}

fn set_oninput(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    set_handler(this, "input", args, context)
}

fn set_onchange(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    set_handler(this, "change", args, context)
}

fn set_onsubmit(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    set_handler(this, "submit", args, context)
}

fn set_handler(this: &JsValue, event: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    let handler = args.first().cloned().unwrap_or(JsValue::undefined());
    push_listener(context, &format!("on:{}:{event}", id.0), &handler, true)?;
    Ok(JsValue::undefined())
}

fn prevent_default(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if let Ok(host) = host(context) {
        host.prevented.set(true);
    }
    Ok(JsValue::undefined())
}

fn event_object(target: NodeId, event: &str, context: &mut Context) -> JsResult<JsValue> {
    let target_value = node_value(target.0, context);
    let current_get = native(context, "get currentTarget", 0, get_current_target);
    let prevented_get = native(context, "get defaultPrevented", 0, get_default_prevented);
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("type"), js_string!(event), ATTR)
        .property(js_string!("target"), target_value, ATTR)
        .property(js_string!("bubbles"), JsValue::from(true), ATTR)
        .accessor(js_string!("currentTarget"), Some(current_get), None, ATTR)
        .accessor(js_string!("defaultPrevented"), Some(prevented_get), None, ATTR)
        .function(NativeFunction::from_fn_ptr(prevent_default), js_string!("preventDefault"), 0);
    Ok(object.build().into())
}

fn listener_functions(context: &mut Context, id: NodeId, event: &str) -> JsResult<Vec<JsObject>> {
    let mut functions = read_listeners(context, &format!("add:{}:{event}", id.0))?;
    functions.extend(read_listeners(context, &format!("on:{}:{event}", id.0))?);
    Ok(functions)
}

fn listener_count(context: &mut Context, key: &str) -> JsResult<usize> {
    Ok(read_listeners(context, key)?.len())
}

fn read_listeners(context: &mut Context, key: &str) -> JsResult<Vec<JsObject>> {
    let global = context.global_object();
    let bag = global.get(js_string!("__mithuListeners"), context)?;
    let Some(bag) = bag.as_object() else {
        return Ok(Vec::new());
    };
    let current = bag.get(js_string!(key), context)?;
    let Some(list) = current.as_object() else {
        return Ok(Vec::new());
    };
    let length = list.get(js_string!("length"), context)?.as_number().unwrap_or(0.0) as usize;
    let mut functions = Vec::new();
    for index in 0..length {
        let item = list.get(js_string!(index.to_string()), context)?;
        if let Some(function) = item.as_object()
            && function.is_callable()
        {
            functions.push(function);
        }
    }
    Ok(functions)
}

fn push_listener(context: &mut Context, key: &str, value: &JsValue, replace: bool) -> JsResult<()> {
    let Some(function) = value.as_object().filter(|object| object.is_callable()) else {
        return Ok(());
    };
    let global = context.global_object();
    let bag = global
        .get(js_string!("__mithuListeners"), context)?
        .as_object()
        .ok_or_else(type_error)?;
    let list = if replace {
        JsObject::from(JsArray::new(context)?)
    } else {
        let current = bag.get(js_string!(key), context)?;
        if let Some(list) = current.as_object().filter(|object| object.is_array()) {
            list
        } else {
            JsObject::from(JsArray::new(context)?)
        }
    };
    let length = list.get(js_string!("length"), context)?.as_number().unwrap_or(0.0);
    list.set(length as u32, function, true, context)?;
    bag.set(js_string!(key), list, true, context)?;
    Ok(())
}

fn run_module(context: &mut Context, code: &str, name: &str) -> JsResult<()> {
    let path = PathBuf::from(name);
    let module = Module::parse(Source::from_bytes(code).with_path(&path), None, context)?;
    let _promise = module.load_link_evaluate(context);
    context.run_jobs()
}

fn run_due_timers(context: &mut Context, ms: u64) -> JsResult<bool> {
    let host = host(context)?;
    {
        let mut clock = host.clock.borrow_mut();
        clock.now = clock.now.saturating_add(ms);
    }
    let mut ran = false;
    for _ in 0..100 {
        let next = {
            let clock = host.clock.borrow();
            clock
                .queue
                .iter()
                .filter(|timer| timer.due <= clock.now)
                .min_by_key(|timer| timer.due)
                .map(|timer| timer.id)
        };
        let Some(id) = next else {
            break;
        };
        ran = true;
        let repeat = {
            let mut clock = host.clock.borrow_mut();
            let repeat = clock.queue.iter().find(|timer| timer.id == id).and_then(|timer| timer.repeat);
            if let Some(every) = repeat {
                let due = clock.now.saturating_add(every);
                if let Some(timer) = clock.queue.iter_mut().find(|timer| timer.id == id) {
                    timer.due = due;
                }
            } else {
                clock.queue.retain(|timer| timer.id != id);
            }
            repeat
        };
        let function = timer_function(context, id)?;
        if let Some(function) = function {
            function.call(&JsValue::undefined(), &[], context)?;
        }
        if repeat.is_none() {
            forget_timer(context, id)?;
        }
        context.run_jobs()?;
    }
    Ok(ran)
}

fn timer_function(context: &mut Context, id: u32) -> JsResult<Option<JsObject>> {
    let global = context.global_object();
    let bag = global.get(js_string!("__mithuTimers"), context)?;
    let Some(bag) = bag.as_object() else {
        return Ok(None);
    };
    let item = bag.get(js_string!(id.to_string()), context)?;
    Ok(item.as_object().filter(|object| object.is_callable()))
}

fn forget_timer(context: &mut Context, id: u32) -> JsResult<()> {
    let global = context.global_object();
    let bag = global.get(js_string!("__mithuTimers"), context)?;
    if let Some(bag) = bag.as_object() {
        bag.set(js_string!(id.to_string()), JsValue::undefined(), true, context)?;
    }
    Ok(())
}

fn schedule_timer(context: &mut Context, args: &[JsValue], repeat: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let function = args.first().cloned().unwrap_or(JsValue::undefined());
    let Some(function) = function.as_object().filter(|object| object.is_callable()) else {
        return Ok(JsValue::from(0));
    };
    let delay = args.get(1).and_then(|value| value.as_number()).unwrap_or(0.0).max(0.0) as u64;
    let delay = if repeat { delay.max(1) } else { delay };
    let id = {
        let mut clock = host.clock.borrow_mut();
        clock.next = clock.next.saturating_add(1);
        let id = clock.next;
        let due = clock.now.saturating_add(delay);
        clock.queue.push(Timer {
            id,
            due,
            repeat: repeat.then_some(delay),
        });
        id
    };
    let global = context.global_object();
    let bag = global
        .get(js_string!("__mithuTimers"), context)?
        .as_object()
        .ok_or_else(type_error)?;
    bag.set(js_string!(id.to_string()), function, true, context)?;
    Ok(JsValue::from(id))
}

fn fetch_resource(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let spec = arg_string(args, 0, context)?;
    let host = host(context)?;
    let location = host.location.borrow().clone();
    let outcome = resolve_request(&location, &spec).and_then(|url| {
        mithu_net::get(&url).map_err(|error| error.to_string()).map(|response| {
            (
                response.status,
                (200..300).contains(&response.status),
                response.url.to_string(),
                response.text(),
            )
        })
    });
    match outcome {
        Ok((status, ok, url, body)) => {
            let response = response_object(status, ok, url, body, context);
            Ok(JsPromise::resolve(response, context)?.into())
        }
        Err(message) => reject_promise(context, &message),
    }
}

fn resolve_request(location: &str, spec: &str) -> Result<mithu_url::Url, String> {
    if let Ok(absolute) = mithu_url::Url::parse(spec) {
        return Ok(absolute);
    }
    let base = mithu_url::Url::parse(location).map_err(|error| error.to_string())?;
    base.resolve(spec).map_err(|error| error.to_string())
}

fn reject_promise(context: &mut Context, message: &str) -> JsResult<JsValue> {
    let (promise, resolvers) = JsPromise::new_pending(context);
    resolvers.reject.call(&JsValue::undefined(), &[js_string!(message).into()], context)?;
    Ok(promise.into())
}

fn response_object(status: u16, ok: bool, url: String, body: String, context: &mut Context) -> JsValue {
    let mut object = ObjectInitializer::with_native_data(ResponseKey { status, ok, url, body }, context);
    object
        .property(js_string!("status"), JsValue::from(i32::from(status)), ATTR)
        .property(js_string!("ok"), JsValue::from(ok), ATTR)
        .function(NativeFunction::from_fn_ptr(response_text), js_string!("text"), 0);
    object.build().into()
}

fn response_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsPromise::resolve(JsValue::undefined(), context)?.into());
    };
    let body = object.downcast_ref::<ResponseKey>().map(|key| key.body.clone()).unwrap_or_default();
    Ok(JsPromise::resolve(js_string!(body), context)?.into())
}

fn contains_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let ancestor = node_of(this).ok_or_else(type_error)?;
    let Some(node) = args.first().and_then(node_of) else {
        return Ok(JsValue::from(false));
    };
    Ok(JsValue::from(host.doc.borrow().contains_node(ancestor, node)))
}

fn dispatch_event(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let target = node_of(this).ok_or_else(type_error)?;
    let event_name = event_type(args.first(), context)?;
    host.prevented.set(false);
    let chain = {
        let doc = host.doc.borrow();
        let mut chain = vec![target];
        let mut current = doc.parent_element(target);
        while let Some(node) = current {
            chain.push(node);
            current = doc.parent_element(node);
        }
        chain
    };
    for current in chain {
        host.event_target.set(Some(target.0));
        host.event_current.set(Some(current.0));
        let functions = listener_functions(context, current, &event_name)?;
        if functions.is_empty() {
            continue;
        }
        let this_value = element_value(current.0, context);
        let event_value = event_object(target, &event_name, context)?;
        for function in functions {
            function.call(&this_value, &[event_value.clone()], context)?;
        }
    }
    Ok(JsValue::from(!host.prevented.get()))
}

fn event_type(value: Option<&JsValue>, context: &mut Context) -> JsResult<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    if let Some(object) = value.as_object() {
        let kind = object.get(js_string!("type"), context)?;
        if !kind.is_undefined() {
            return Ok(kind.to_string(context)?.to_std_string_lossy());
        }
    }
    Ok(value.to_string(context)?.to_std_string_lossy())
}

fn set_timeout(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer(context, args, false)
}

fn set_interval(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer(context, args, true)
}

fn clear_timer(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = args.first().and_then(|value| value.as_number()).unwrap_or(0.0) as u32;
    host.clock.borrow_mut().queue.retain(|timer| timer.id != id);
    forget_timer(context, id)?;
    Ok(JsValue::undefined())
}

fn window_listener(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    push_listener(context, &format!("win:{event}"), &handler, false)?;
    Ok(JsValue::undefined())
}

fn document_listener(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let Some(html) = find_tag(&host.doc.borrow(), "html") else {
        return Ok(JsValue::undefined());
    };
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    push_listener(context, &format!("add:{}:{event}", html.0), &handler, false)?;
    Ok(JsValue::undefined())
}

fn get_location(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(js_string!(host.location.borrow().clone()).into())
}

fn set_location(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let next = arg_string(args, 0, context)?;
    if host.location.borrow().as_str() != next {
        *host.navigation.borrow_mut() = Some(next.clone());
        *host.location.borrow_mut() = next;
    }
    Ok(JsValue::undefined())
}

fn get_document_element(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(find_tag(&host.doc.borrow(), "html").map(|id| element_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn get_head(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(find_tag(&host.doc.borrow(), "head").map(|id| element_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn create_text_node(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = arg_string(args, 0, context)?;
    let id = host.doc.borrow_mut().create_text_detached(&text);
    Ok(node_value(id.0, context))
}

fn node_value(id: usize, context: &mut Context) -> JsValue {
    let element = host(context).ok().is_some_and(|host| host.doc.borrow().element_name(NodeId(id)).is_some());
    if element {
        element_value(id, context)
    } else {
        text_node(id, context)
    }
}

fn text_node(id: usize, context: &mut Context) -> JsValue {
    let type_get = native(context, "get nodeType", 0, get_node_type);
    let name_get = native(context, "get nodeName", 0, get_node_name);
    let text_get = native(context, "get textContent", 0, get_text);
    let text_set = native(context, "set textContent", 1, set_text);
    let parent_get = native(context, "get parentNode", 0, get_parent_node);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("nodeType"), Some(type_get), None, ATTR)
        .accessor(js_string!("nodeName"), Some(name_get), None, ATTR)
        .accessor(js_string!("textContent"), Some(text_get), Some(text_set), ATTR)
        .accessor(js_string!("parentNode"), Some(parent_get), None, ATTR);
    object.build().into()
}

fn get_node_type(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let kind = match &host.doc.borrow().node(id).data {
        NodeData::Element(_) => 1,
        NodeData::Text(_) => 3,
        NodeData::Comment(_) => 8,
        NodeData::Document => 9,
    };
    Ok(JsValue::from(kind))
}

fn get_node_name(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = match &host.doc.borrow().node(id).data {
        NodeData::Element(element) => element.name.to_ascii_uppercase(),
        NodeData::Text(_) => "#text".to_string(),
        NodeData::Comment(_) => "#comment".to_string(),
        NodeData::Document => "#document".to_string(),
    };
    Ok(js_string!(name).into())
}

fn get_value(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    attr_value(this, "value", context)
}

fn set_value(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    write_attr(this, "value", args, context)
}

fn get_href(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    attr_value(this, "href", context)
}

fn set_href(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    write_attr(this, "href", args, context)
}

fn attr_value(this: &JsValue, name: &str, context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, name).unwrap_or("").to_string()).into())
}

fn write_attr(this: &JsValue, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, name, &value);
    Ok(JsValue::undefined())
}

fn get_checked(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(JsValue::from(host.doc.borrow().attr(id, "checked").is_some()))
}

fn set_checked(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let on = args.first().and_then(|value| value.as_boolean()).unwrap_or(false);
    let mut doc = host.doc.borrow_mut();
    if on {
        doc.set_attr(id, "checked", "");
    } else {
        doc.remove_attr(id, "checked");
    }
    Ok(JsValue::undefined())
}

fn remove_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    host.doc.borrow_mut().remove_attr(id, &name);
    Ok(JsValue::undefined())
}

fn has_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    Ok(JsValue::from(host.doc.borrow().attr(id, &name).is_some()))
}

fn matches_selector(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let selector = arg_string(args, 0, context)?;
    Ok(JsValue::from(selector_hits(&host.doc.borrow(), id, &selector)))
}

fn closest_selector(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let found = {
        let doc = host.doc.borrow();
        let mut current = node_of(this);
        let mut found = None;
        while let Some(id) = current {
            if selector_hits(&doc, id, &selector) {
                found = Some(id);
                break;
            }
            current = doc.parent_element(id);
        }
        found
    };
    Ok(found.map(|id| element_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn selector_hits(doc: &Document, id: NodeId, selector: &str) -> bool {
    if doc.element_name(id).is_none() {
        return false;
    }
    let sheet = parse_css(&format!("{selector}{{}}"));
    sheet.rules.iter().flat_map(|rule| &rule.selectors).any(|selector| matches_element(doc, id, selector))
}

fn insert_before_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    let before = args.get(1).and_then(node_of);
    host.doc.borrow_mut().insert_before(parent, child, before);
    Ok(child_value)
}

fn remove_event_listener(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    drop_listener(context, &format!("add:{}:{event}", id.0), &handler)?;
    Ok(JsValue::undefined())
}

fn get_child_nodes(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    listed_children(this, context, false)
}

fn get_first_child(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    edge_child(this, context, true)
}

fn get_last_child(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    edge_child(this, context, false)
}

fn edge_child(this: &JsValue, context: &mut Context, first: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let children = host.doc.borrow().node(id).children.clone();
    let child = if first { children.first().copied() } else { children.last().copied() };
    Ok(child.map(|id| node_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn get_next_node(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    sibling_node(this, context, true)
}

fn get_previous_node(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    sibling_node(this, context, false)
}

fn sibling_node(this: &JsValue, context: &mut Context, next: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let sibling = {
        let doc = host.doc.borrow();
        let Some(parent) = doc.node(id).parent else {
            return Ok(JsValue::null());
        };
        let children = &doc.node(parent).children;
        let Some(index) = children.iter().position(|child| *child == id) else {
            return Ok(JsValue::null());
        };
        if next {
            children.get(index + 1).copied()
        } else {
            index.checked_sub(1).and_then(|index| children.get(index).copied())
        }
    };
    Ok(sibling.map(|id| node_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn get_parent_node(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let parent = {
        let doc = host.doc.borrow();
        doc.node(id).parent.filter(|parent| doc.element_name(*parent).is_some())
    };
    Ok(parent.map(|id| element_value(id.0, context)).unwrap_or(JsValue::null()))
}

fn get_current_target(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(host.event_current.get().map(|id| node_value(id, context)).unwrap_or(JsValue::null()))
}

fn get_default_prevented(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(JsValue::from(host.prevented.get()))
}

fn listed_children(this: &JsValue, context: &mut Context, elements_only: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let children = host.doc.borrow().node(id).children.clone();
    let values = children
        .into_iter()
        .filter(|child| !elements_only || host.doc.borrow().element_name(*child).is_some())
        .map(|child| node_value(child.0, context))
        .collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn drop_listener(context: &mut Context, key: &str, value: &JsValue) -> JsResult<()> {
    let kept = read_listeners(context, key)?
        .into_iter()
        .filter(|function| &JsValue::from(function.clone()) != value)
        .map(JsValue::from)
        .collect::<Vec<_>>();
    let global = context.global_object();
    let bag = global
        .get(js_string!("__mithuListeners"), context)?
        .as_object()
        .ok_or_else(type_error)?;
    bag.set(js_string!(key), JsObject::from(JsArray::from_iter(kept, context)), true, context)?;
    Ok(())
}

impl ModuleLoader for PageLoader {
    fn load_imported_module(
        self: Rc<Self>,
        _referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl std::future::Future<Output = JsResult<Module>> {
        let specifier = request.specifier().to_std_string_lossy();
        let result = (|| {
            if let Some(module) = self.cache.borrow().get(&specifier).cloned() {
                return Ok(module);
            }
            let code = (self.fetch.borrow_mut())(&specifier).ok_or_else(|| {
                JsError::from(JsNativeError::typ().with_message(format!("module not found: {specifier}")))
            })?;
            let path = PathBuf::from(&specifier);
            let module = {
                let mut context = context.borrow_mut();
                Module::parse(Source::from_bytes(code.as_str()).with_path(&path), None, &mut context)?
            };
            self.cache.borrow_mut().insert(specifier, module.clone());
            Ok(module)
        })();
        std::future::ready(result)
    }
}

fn form_owner(doc: &Document, id: NodeId) -> Option<NodeId> {
    if doc.element_name(id) == Some("form") {
        return Some(id);
    }
    let mut current = doc.parent_element(id);
    while let Some(node) = current {
        if doc.element_name(node) == Some("form") {
            return Some(node);
        }
        current = doc.parent_element(node);
    }
    None
}

fn form_query(doc: &Document, form: NodeId) -> String {
    let mut pairs = Vec::new();
    collect_controls(doc, form, form, &mut pairs);
    pairs.join("&")
}

fn collect_controls(doc: &Document, form: NodeId, id: NodeId, pairs: &mut Vec<String>) {
    for child in doc.node(id).children.clone() {
        if doc.element_name(child) == Some("form") && child != form {
            continue;
        }
        if let Some(pair) = control_pair(doc, child) {
            pairs.push(pair);
        }
        collect_controls(doc, form, child, pairs);
    }
}

fn control_pair(doc: &Document, id: NodeId) -> Option<String> {
    let name = doc.element_name(id)?;
    let key = doc.attr(id, "name")?;
    if key.is_empty() || doc.attr(id, "disabled").is_some() {
        return None;
    }
    let value = match name {
        "textarea" => doc.text_content(id),
        "select" => doc.text_content(id),
        "input" => {
            let kind = doc.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
            if matches!(kind.as_str(), "submit" | "button" | "reset" | "image" | "file") {
                return None;
            }
            if matches!(kind.as_str(), "checkbox" | "radio") && doc.attr(id, "checked").is_none() {
                return None;
            }
            if kind == "checkbox" {
                doc.attr(id, "value").unwrap_or("on").to_string()
            } else {
                doc.attr(id, "value").unwrap_or("").to_string()
            }
        }
        _ => return None,
    };
    Some(format!("{}={}", form_encode(key), form_encode(&value)))
}

fn form_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(*byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn host(context: &mut Context) -> JsResult<Host> {
    context.get_data::<Host>().cloned().ok_or_else(type_error)
}

fn type_error() -> JsError {
    JsNativeError::typ().with_message("DOM object is missing").into()
}

fn arg_string(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<String> {
    let value = args.get(index).cloned().unwrap_or(JsValue::undefined());
    Ok(value.to_string(context)?.to_std_string_lossy())
}

fn node_of(value: &JsValue) -> Option<NodeId> {
    let object = value.as_object()?;
    let key = object.downcast_ref::<NodeKey>()?;
    Some(NodeId(key.id))
}

fn find_tag(doc: &Document, name: &str) -> Option<NodeId> {
    (0..doc.nodes.len()).map(NodeId).find(|id| doc.element_name(*id) == Some(name))
}

fn query_first(doc: &Document, scope: NodeId, selector: &str, descendants_only: bool) -> Option<NodeId> {
    query_all(doc, scope, selector, descendants_only).into_iter().next()
}

fn query_all(doc: &Document, scope: NodeId, selector: &str, descendants_only: bool) -> Vec<NodeId> {
    let sheet = parse_css(&format!("{selector} {{ color: black }}"));
    let Some(parsed) = sheet.rules.first().and_then(|rule| rule.selectors.first()) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    if !descendants_only && doc.element_name(scope).is_some() {
        nodes.push(scope);
    }
    collect_elements(doc, scope, &mut nodes);
    nodes.into_iter().filter(|id| matches_element(doc, *id, parsed)).collect()
}

fn collect_elements(doc: &Document, id: NodeId, out: &mut Vec<NodeId>) {
    let Some(node) = doc.nodes.get(id.0) else {
        return;
    };
    for child in node.children.clone() {
        if doc.element_name(child).is_some() {
            out.push(child);
            collect_elements(doc, child, out);
        }
    }
}

fn copy_children(src: &Document, src_id: NodeId, dst: &mut Document, dst_parent: NodeId) {
    for child in src.node(src_id).children.clone() {
        match &src.node(child).data {
            NodeData::Text(text) => {
                dst.append_text(dst_parent, text);
            }
            NodeData::Element(element) => {
                let id = dst.create_element(dst_parent, &element.name);
                for (name, value) in &element.attrs {
                    dst.set_attr(id, name, value);
                }
                copy_children(src, child, dst, id);
            }
            NodeData::Comment(_) | NodeData::Document => {}
        }
    }
}

fn serialize(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    for child in &doc.node(id).children {
        write_html(doc, *child, &mut out);
    }
    out
}

fn write_html(doc: &Document, id: NodeId, out: &mut String) {
    match &doc.node(id).data {
        NodeData::Text(text) => out.push_str(&escape(text)),
        NodeData::Element(element) => {
            out.push('<');
            out.push_str(&element.name);
            for (name, value) in &element.attrs {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape(value));
                out.push('"');
            }
            out.push('>');
            for child in &doc.node(id).children {
                write_html(doc, *child, out);
            }
            out.push_str("</");
            out.push_str(&element.name);
            out.push('>');
        }
        NodeData::Comment(text) => {
            out.push_str("<!--");
            out.push_str(text);
            out.push_str("-->");
        }
        NodeData::Document => {}
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithu_html::parse_html;

    #[test]
    fn script_updates_text_and_style() {
        let mut doc = parse_html(
            r##"<p id="msg">before</p><div id="host"></div>
            <script>
              document.getElementById("msg").textContent = "after";
              document.title = "Changed";
              var el = document.querySelector("#host");
              el.className = "on";
              el.style.color = "navy";
              el.style.display = "flex";
              console.log("ready");
            </script>"##,
        );
        let log = run_scripts(&mut doc, |_| None);
        assert_eq!(doc.get_element_by_id("msg").map(|id| doc.text_content(id)).as_deref(), Some("after"));
        assert_eq!(doc.title, "Changed");
        let host_node = doc.get_element_by_id("host").unwrap();
        assert!(doc.has_class(host_node, "on"));
        let style = doc.attr(host_node, "style").unwrap_or("");
        assert!(style.contains("color: navy"), "{style}");
        assert!(style.contains("display: flex"), "{style}");
        assert_eq!(log, vec!["ready".to_string()]);
    }

    #[test]
    fn inner_html_builds_elements() {
        let mut doc = parse_html(r##"<div id="host"></div><script>
            document.getElementById("host").innerHTML = "<b id='yes'>Hi</b>";
        </script>"##);
        run_scripts(&mut doc, |_| None);
        let yes = doc.get_element_by_id("yes").unwrap();
        assert_eq!(doc.text_content(yes), "Hi");
        assert_eq!(doc.element_name(yes), Some("b"));
    }

    #[test]
    fn click_listener_and_prevent_default() {
        let doc = parse_html(
            r##"<button id="go">Go</button><p id="out" onclick="this.textContent='attr'">old</p>
            <a id="link" href="https://example.com/">Example</a>
            <script>
              document.getElementById("go").addEventListener("click", function () {
                document.getElementById("out").textContent = "clicked";
              });
              document.getElementById("link").addEventListener("click", function (event) {
                event.preventDefault();
              });
            </script>"##,
        );
        let mut session = Session::open(doc, |_| None);
        let go = session.document().get_element_by_id("go").unwrap();
        assert!(session.dispatch(go, "click"));
        let out = session.document().get_element_by_id("out").unwrap();
        assert_eq!(session.document().text_content(out), "clicked");
        assert!(session.dispatch(out, "click"));
        assert_eq!(session.document().text_content(out), "attr");
        let link = session.document().get_element_by_id("link").unwrap();
        assert!(!session.dispatch(link, "click"));
    }

    #[test]
    fn form_submission_encodes_fields() {
        let doc = parse_html(
            r#"<form id="find" action="/search" method="get">
                <input name="q" value="mithu browser">
                <input type="checkbox" name="show" value="yes" checked>
                <input type="checkbox" name="hide" value="no">
            </form>"#,
        );
        let mut session = Session::open(doc, |_| None);
        let form = session.document().get_element_by_id("find").unwrap();
        let submission = session.submission(form).expect("submit");
        assert_eq!(submission.action, "/search");
        assert_eq!(submission.method, "get");
        assert_eq!(submission.query, "q=mithu+browser&show=yes");
    }

    #[test]
    fn class_list_and_element_tree() {
        let mut doc = parse_html(
            r##"<div id="box"><span id="a"></span><span id="b"></span></div>
            <script>
              var box = document.getElementById("box");
              box.classList.add("one", "two");
              box.classList.toggle("one");
              var kids = box.children;
              kids[1].parentElement.removeChild(kids[1]);
            </script>"##,
        );
        run_scripts(&mut doc, |_| None);
        let host = doc.get_element_by_id("box").unwrap();
        assert!(doc.has_class(host, "two"));
        assert!(!doc.has_class(host, "one"));
        assert!(doc.parent_element(doc.get_element_by_id("b").unwrap()).is_none());
        assert_eq!(doc.parent_element(doc.get_element_by_id("a").unwrap()), Some(host));
    }

    #[test]
    fn language_promises_classes_and_modules() {
        let mut doc = parse_html(
            r##"<p id="out"></p>
            <script>
              class Box { constructor(v) { this.v = v } get twice() { return this.v * 2 } }
              const box = new Box(3);
              const mapped = [1, 2, 3].map((n) => n + 1);
              const unique = new Set([1, 1, 2]);
              const table = new Map([["a", 1]]);
              const text = `n=${box.twice}`;
              const fallback = null?.missing ?? 4;
              const { a, ...rest } = { a: 1, b: 2 };
              const parsed = JSON.parse('{"ok":true}');
              Promise.resolve(5).then((value) => { globalThis.promised = value; });
              document.getElementById("out").textContent = [text, mapped.join(""), unique.size, table.get("a"), fallback, rest.b, parsed.ok].join(":");
            </script>
            <script type="module">
              import { value } from "./lib.js";
              document.title = value;
            </script>"##,
        );
        let log = run_scripts(&mut doc, |src| {
            (src == "./lib.js").then(|| "export const value = \"imported\";".to_string())
        });
        assert!(log.is_empty(), "{log:?}");
        let out = doc.get_element_by_id("out").unwrap();
        assert_eq!(doc.text_content(out), "n=6:234:2:1:4:2:true");
        assert_eq!(doc.title, "imported");
    }

    #[test]
    fn timers_dom_and_location_round_trip() {
        let doc = parse_html(
            r##"<div id="box" class="a"><span id="child">Hi</span></div>
            <script>
              var box = document.querySelector("#box");
              box.matches(".a") && box.closest("div");
              var text = document.createTextNode("!");
              box.insertBefore(text, box.firstChild);
              var seen = false;
              function onClick() { seen = true; }
              box.addEventListener("click", onClick);
              box.removeEventListener("click", onClick);
              window.addEventListener("click", function () { document.title = "win"; });
              setTimeout(function () { box.value = "later"; }, 20);
              location.href = "https://example.com/next";
            </script>"##,
        );
        let mut session = Session::open_at(doc, "https://example.com/", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.take_navigation().as_deref(), Some("https://example.com/next"));
        let document = session.document();
        let host_node = document.get_element_by_id("box").unwrap();
        assert_eq!(document.node(host_node).children.len(), 2);
        assert!(!session.advance(10));
        assert!(session.advance(10));
        let document = session.document();
        let host_node = document.get_element_by_id("box").unwrap();
        assert_eq!(document.attr(host_node, "value"), Some("later"));
        session.dispatch(host_node, "click");
        assert_eq!(session.document().title, "win");
    }

    #[test]
    fn fetch_rejects_a_non_network_url_and_dispatch_bubbles() {
        let mut doc = parse_html(
            r##"<div id="box"><b id="inner">x</b></div>
            <script>
              var box = document.getElementById("box");
              var inner = document.getElementById("inner");
              box.addEventListener("ping", function () { box.className = "hit"; });
              inner.dispatchEvent({ type: "ping" });
              document.title = String(box.contains(inner));
              fetch("about:blank").then(function () {}, function () { box.value = "no"; });
            </script>"##,
        );
        let log = run_scripts(&mut doc, |_| None);
        assert!(log.is_empty(), "{log:?}");
        let host = doc.get_element_by_id("box").unwrap();
        assert!(doc.has_class(host, "hit"));
        assert_eq!(doc.title, "true");
        assert_eq!(doc.attr(host, "value"), Some("no"));
    }
}
