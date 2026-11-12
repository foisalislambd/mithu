//! In-process JavaScript virtual machine.
//!
//! Boa runs ECMAScript in this process, including modules. Host objects are the
//! window, document, and node tree. Timers and `location` report back to the browser.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

use boa_engine::module::{Module, ModuleLoader, ModuleRequest, Referrer};
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::script::Script;
use boa_engine::{
    Context, Finalize, JsData, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue,
    NativeFunction, Source, Trace, js_string,
};
use mithu_css::{Declaration, parse_css, parse_declarations};
use mithu_dom::{Document, NodeData, NodeId};
use mithu_html::parse_html;
use mithu_style::matches_element;

const ATTR: Attribute = Attribute::WRITABLE
    .union(Attribute::ENUMERABLE)
    .union(Attribute::CONFIGURABLE);

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
    local: Rc<RefCell<HashMap<String, String>>>,
    session_store: Rc<RefCell<HashMap<String, String>>>,
    history: Rc<RefCell<Vec<String>>>,
    history_index: Rc<Cell<usize>>,
    current_script: Rc<RefCell<Option<NodeId>>>,
    ready_state: Rc<RefCell<String>>,
    geometry: Rc<RefCell<HashMap<usize, BoxMetrics>>>,
    scroll: Rc<Cell<f32>>,
}

/// Border box of a laid-out element, in CSS pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoxMetrics {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub border_top: f32,
    pub border_right: f32,
    pub border_bottom: f32,
    pub border_left: f32,
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
    /// 0 = callback, 1 = idle callback, 2 = async XHR completion.
    kind: u8,
}

struct PageLoader {
    fetch: RefCell<Box<dyn FnMut(&str) -> Option<String>>>,
    cache: RefCell<HashMap<String, Module>>,
    imports: HashMap<String, String>,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct NodeKey {
    #[unsafe_ignore_trace]
    id: usize,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct ResponseKey {
    #[unsafe_ignore_trace]
    body: String,
    #[unsafe_ignore_trace]
    headers: String,
}

pub fn engine_name() -> &'static str {
    "boa"
}

pub struct Session {
    /// `None` while a script task owns the engine. The document stays readable.
    context: Option<Context>,
    driver: Option<Pin<Box<dyn Future<Output = Context>>>>,
    doc: Rc<RefCell<Document>>,
    log: Rc<RefCell<Vec<String>>>,
    prevented: Rc<Cell<bool>>,
    local: Rc<RefCell<HashMap<String, String>>>,
    session_store: Rc<RefCell<HashMap<String, String>>>,
    geometry: Rc<RefCell<HashMap<usize, BoxMetrics>>>,
    scroll: Rc<Cell<f32>>,
}

struct ScriptJob {
    code: String,
    path: String,
    module: bool,
    node: NodeId,
}

impl Session {
    pub fn open(doc: Document, fetch: impl FnMut(&str) -> Option<String> + 'static) -> Self {
        Self::open_at(doc, "about:blank", fetch)
    }

    pub fn open_at(
        doc: Document,
        href: &str,
        fetch: impl FnMut(&str) -> Option<String> + 'static,
    ) -> Self {
        Self::open_with_storage(doc, href, fetch, HashMap::new(), HashMap::new())
    }

    /// Like [`Session::open_with_storage`], but script tasks stay queued.
    /// [`Session::pump`] runs them in slices so the UI thread can paint.
    pub fn open_cooperative(
        doc: Document,
        href: &str,
        fetch: impl FnMut(&str) -> Option<String> + 'static,
        local: HashMap<String, String>,
        session_store: HashMap<String, String>,
    ) -> Self {
        Self::boot(doc, href, fetch, local, session_store, false)
    }

    pub fn open_with_storage(
        doc: Document,
        href: &str,
        fetch: impl FnMut(&str) -> Option<String> + 'static,
        local: HashMap<String, String>,
        session_store: HashMap<String, String>,
    ) -> Self {
        Self::boot(doc, href, fetch, local, session_store, true)
    }

    fn boot(
        doc: Document,
        href: &str,
        fetch: impl FnMut(&str) -> Option<String> + 'static,
        local: HashMap<String, String>,
        session_store: HashMap<String, String>,
        finish: bool,
    ) -> Self {
        let log = Rc::new(RefCell::new(Vec::new()));
        let prevented = Rc::new(Cell::new(false));
        let imports = import_map(&doc);
        let shared = Rc::new(RefCell::new(doc));
        let location = Rc::new(RefCell::new(href.to_string()));
        let navigation = Rc::new(RefCell::new(None));
        let clock = Rc::new(RefCell::new(Clock {
            now: 0,
            next: 0,
            queue: Vec::new(),
        }));
        let loader = Rc::new(PageLoader {
            fetch: RefCell::new(Box::new(fetch)),
            cache: RefCell::new(HashMap::new()),
            imports,
        });
        let local = Rc::new(RefCell::new(local));
        let session_store = Rc::new(RefCell::new(session_store));
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
            local: Rc::clone(&local),
            session_store: Rc::clone(&session_store),
            history: Rc::new(RefCell::new(vec![href.to_string()])),
            history_index: Rc::new(Cell::new(0)),
            current_script: Rc::new(RefCell::new(None)),
            ready_state: Rc::new(RefCell::new("interactive".to_string())),
            geometry: Rc::new(RefCell::new(HashMap::new())),
            scroll: Rc::new(Cell::new(0.0)),
        });
        let mut jobs = Vec::new();
        if let Err(error) = install_host(&mut context) {
            log.borrow_mut().push(format!("JavaScript error: {error}"));
        } else {
            jobs = script_jobs(&context, &shared, &loader);
        }
        let geometry = context
            .get_data::<Host>()
            .map(|host| Rc::clone(&host.geometry))
            .unwrap_or_else(|| Rc::new(RefCell::new(HashMap::new())));
        let scroll = context
            .get_data::<Host>()
            .map(|host| Rc::clone(&host.scroll))
            .unwrap_or_else(|| Rc::new(Cell::new(0.0)));
        let (context, driver) = if jobs.is_empty() {
            signal_document_loaded(&mut context);
            (Some(context), None)
        } else {
            (None, Some(drive_scripts(context, jobs)))
        };
        let mut session = Self {
            context,
            driver,
            doc: shared,
            log,
            prevented,
            local,
            session_store,
            geometry,
            scroll,
        };
        if finish {
            while session.pump(Duration::from_secs(60)) {}
        }
        session
    }

    /// True while script tasks are still queued. Blink keeps the renderer
    /// responsive by returning to the event loop between slices.
    pub fn scripts_pending(&self) -> bool {
        self.driver.is_some()
    }

    /// Run the current script task until `budget` elapses or the queue drains.
    /// Returns true when more script work remains.
    ///
    /// Boa yields every 16_384 instruction-costs (`Script::evaluate_async_with_budget`).
    /// That is the same idea as Blink's per-task budget: a long bundle is one
    /// task, but the thread can paint between yields.
    pub fn pump(&mut self, budget: Duration) -> bool {
        let Some(mut driver) = self.driver.take() else {
            return false;
        };
        let start = Instant::now();
        let waker = Waker::noop();
        let mut task = TaskContext::from_waker(waker);
        loop {
            match driver.as_mut().poll(&mut task) {
                Poll::Ready(context) => {
                    self.context = Some(context);
                    return false;
                }
                Poll::Pending => {
                    if start.elapsed() >= budget {
                        self.driver = Some(driver);
                        return true;
                    }
                }
            }
        }
    }

    pub fn local_storage(&self) -> HashMap<String, String> {
        self.local.borrow().clone()
    }

    pub fn session_storage(&self) -> HashMap<String, String> {
        self.session_store.borrow().clone()
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

    /// Remember the latest layout so `getBoundingClientRect` and `offsetWidth` can read it.
    pub fn set_geometry(&self, boxes: HashMap<usize, BoxMetrics>, scroll: f32) {
        *self.geometry.borrow_mut() = boxes;
        self.scroll.set(scroll);
    }

    pub fn take_navigation(&mut self) -> Option<String> {
        self.context
            .as_mut()?
            .get_data::<Host>()
            .cloned()
            .and_then(|host| host.navigation.borrow_mut().take())
    }

    /// Moves the page clock forward and runs timers whose delay has elapsed.
    pub fn advance(&mut self, ms: u64) -> bool {
        let Some(context) = self.context.as_mut() else {
            return false;
        };
        match run_due_timers(context, ms) {
            Ok(ran) => ran,
            Err(error) => {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
                false
            }
        }
    }

    pub fn next_timer_ms(&self) -> Option<u64> {
        let host = self.context.as_ref()?.get_data::<Host>().cloned()?;
        let clock = host.clock.borrow();
        clock
            .queue
            .iter()
            .map(|timer| timer.due.saturating_sub(clock.now))
            .min()
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
        let method = doc
            .attr(form, "method")
            .unwrap_or("get")
            .to_ascii_lowercase();
        let method = if method == "post" { "post" } else { "get" };
        Some(Submission {
            action,
            method: method.to_string(),
            query: form_query(&doc, form),
        })
    }

    fn invoke(&mut self, current: NodeId, target: NodeId, event: &str) {
        if self.context.is_none() {
            return;
        }
        self.ensure_attribute_handler(current, event);
        let Some(context) = self.context.as_mut() else {
            return;
        };
        let functions = match listener_functions(context, current, event) {
            Ok(functions) => functions,
            Err(error) => {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
                return;
            }
        };
        if functions.is_empty() {
            return;
        }
        if let Some(host) = self
            .context
            .as_mut()
            .and_then(|context| context.get_data::<Host>().cloned())
        {
            host.event_target.set(Some(target.0));
            host.event_current.set(Some(current.0));
        }
        let Some(context) = self.context.as_mut() else {
            return;
        };
        let this = element_value(current.0, context);
        let event_value = match event_object(target, event, context) {
            Ok(value) => value,
            Err(error) => {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
                return;
            }
        };
        let Some(context) = self.context.as_mut() else {
            return;
        };
        for function in functions {
            if let Err(error) = function.call(&this, &[event_value.clone()], context) {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
            }
        }
    }

    fn invoke_window(&mut self, target: NodeId, event: &str) {
        if self.context.is_none() {
            return;
        }
        if let Some(host) = self
            .context
            .as_mut()
            .and_then(|context| context.get_data::<Host>().cloned())
        {
            host.event_target.set(Some(target.0));
            host.event_current.set(None);
        }
        let Some(context) = self.context.as_mut() else {
            return;
        };
        let functions = match read_listeners(context, &format!("win:{event}")) {
            Ok(functions) => functions,
            Err(error) => {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
                return;
            }
        };
        if functions.is_empty() {
            return;
        }
        let Some(context) = self.context.as_mut() else {
            return;
        };
        let event_value = match event_object(target, event, context) {
            Ok(value) => value,
            Err(error) => {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
                return;
            }
        };
        let Some(context) = self.context.as_mut() else {
            return;
        };
        let this = JsValue::from(context.global_object().clone());
        for function in functions {
            if let Err(error) = function.call(&this, &[event_value.clone()], context) {
                self.log
                    .borrow_mut()
                    .push(format!("JavaScript error: {error}"));
            }
        }
    }

    fn ensure_attribute_handler(&mut self, id: NodeId, event: &str) {
        let code = {
            let doc = self.doc.borrow();
            doc.attr(id, &format!("on{event}"))
                .unwrap_or("")
                .to_string()
        };
        if code.trim().is_empty() {
            return;
        }
        let key = format!("on:{}:{event}", id.0);
        let Some(context) = self.context.as_mut() else {
            return;
        };
        if listener_count(context, &key).unwrap_or(0) > 0 {
            return;
        }
        let source = format!("(function(event) {{\n{code}\n}})");
        let Some(context) = self.context.as_mut() else {
            return;
        };
        match context.eval(Source::from_bytes(source.as_str())) {
            Ok(value) => {
                let Some(context) = self.context.as_mut() else {
                    return;
                };
                if let Err(error) = push_listener(context, &key, &value, false) {
                    self.log
                        .borrow_mut()
                        .push(format!("JavaScript error: {error}"));
                }
            }
            Err(error) => self
                .log
                .borrow_mut()
                .push(format!("JavaScript error: {error}")),
        }
    }
}

pub struct Submission {
    pub action: String,
    pub method: String,
    pub query: String,
}

fn script_jobs(
    context: &Context,
    doc: &Rc<RefCell<Document>>,
    loader: &PageLoader,
) -> Vec<ScriptJob> {
    let scripts = doc.borrow().scripts.clone();
    let mut classic = Vec::new();
    let mut deferred = Vec::new();
    let mut asynchronous = Vec::new();
    for script in scripts {
        if !script.javascript || script.nomodule {
            continue;
        }
        if script.module || script.defer {
            deferred.push(script);
        } else if script.asynchronous {
            asynchronous.push(script);
        } else {
            classic.push(script);
        }
    }
    let mut jobs = Vec::new();
    for script in classic.into_iter().chain(deferred).chain(asynchronous) {
        let code = if let Some(src) = &script.src {
            let mut fetch = loader.fetch.borrow_mut();
            fetch(src).unwrap_or_default()
        } else {
            script.code.clone()
        };
        if code.trim().is_empty() {
            continue;
        }
        let path = script
            .src
            .as_deref()
            .map(|src| {
                let location = context
                    .get_data::<Host>()
                    .map(|host| host.location.borrow().clone())
                    .unwrap_or_default();
                resolve_request(&location, src)
                    .map(|url| url.to_display())
                    .unwrap_or_else(|_| src.to_string())
            })
            .unwrap_or_else(|| {
                if script.module {
                    "inline-module.js".to_string()
                } else {
                    "inline.js".to_string()
                }
            });
        jobs.push(ScriptJob {
            code,
            path,
            module: script.module,
            node: script.node,
        });
    }
    jobs
}

fn note_error(context: &Context, error: &dyn std::fmt::Display) {
    if let Some(host) = context.get_data::<Host>() {
        host.log
            .borrow_mut()
            .push(format!("JavaScript error: {error}"));
    }
}

/// One script is one task. Boa suspends the task every 16_384 instruction
/// costs so the browser can paint without aborting the bundle.
fn drive_scripts(
    mut context: Context,
    jobs: Vec<ScriptJob>,
) -> Pin<Box<dyn Future<Output = Context>>> {
    Box::pin(async move {
        for job in jobs {
            if let Some(host) = context.get_data::<Host>() {
                *host.current_script.borrow_mut() = Some(job.node);
            }
            if job.module {
                if let Err(error) = run_module(&mut context, &job.code, &job.path) {
                    note_error(&context, &error);
                }
            } else {
                let path = PathBuf::from(&job.path);
                match Script::parse(
                    Source::from_bytes(job.code.as_str()).with_path(&path),
                    None,
                    &mut context,
                ) {
                    Ok(script) => {
                        if let Err(error) = script
                            .evaluate_async_with_budget(&mut context, 16_384)
                            .await
                        {
                            note_error(&context, &error);
                        } else if let Err(error) = context.run_jobs() {
                            note_error(&context, &error);
                        }
                    }
                    Err(error) => note_error(&context, &error),
                }
            }
            if let Some(host) = context.get_data::<Host>() {
                *host.current_script.borrow_mut() = None;
            }
            if let Err(error) = deliver_mutations(&mut context) {
                note_error(&context, &error);
            }
        }
        if let Err(error) = run_due_timers(&mut context, 0) {
            note_error(&context, &error);
        }
        signal_document_loaded(&mut context);
        context
    })
}

pub fn run_scripts(
    doc: &mut Document,
    fetch: impl FnMut(&str) -> Option<String> + 'static,
) -> Vec<String> {
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
    let nodes = ObjectInitializer::new(context).build();
    let window = context.global_object().clone();
    context.register_global_property(js_string!("__mithuListeners"), listeners, flags)?;
    context.register_global_property(js_string!("__mithuNodes"), nodes, flags)?;
    context.register_global_property(js_string!("__mithuTimers"), timers, flags)?;
    context.register_global_property(js_string!("document"), document, flags)?;
    context.register_global_property(js_string!("console"), console, flags)?;
    let navigator = navigator_object(context)?;
    let location = location_object(context)?;
    context.register_global_property(js_string!("window"), window.clone(), flags)?;
    context.register_global_property(js_string!("self"), window, flags)?;
    let performance = performance_object(context)?;
    context.register_global_property(js_string!("performance"), performance, flags)?;
    context.register_global_property(js_string!("navigator"), navigator, flags)?;
    context.register_global_property(js_string!("location"), location, flags)?;
    context.register_global_callable(
        js_string!("setTimeout"),
        2,
        NativeFunction::from_fn_ptr(set_timeout),
    )?;
    context.register_global_callable(
        js_string!("requestAnimationFrame"),
        1,
        NativeFunction::from_fn_ptr(request_animation_frame),
    )?;
    context.register_global_callable(
        js_string!("cancelAnimationFrame"),
        1,
        NativeFunction::from_fn_ptr(clear_timer),
    )?;
    context.register_global_callable(
        js_string!("setInterval"),
        2,
        NativeFunction::from_fn_ptr(set_interval),
    )?;
    context.register_global_callable(
        js_string!("clearTimeout"),
        1,
        NativeFunction::from_fn_ptr(clear_timer),
    )?;
    context.register_global_callable(
        js_string!("clearInterval"),
        1,
        NativeFunction::from_fn_ptr(clear_timer),
    )?;
    context.register_global_callable(
        js_string!("addEventListener"),
        2,
        NativeFunction::from_fn_ptr(window_listener),
    )?;
    context.register_global_callable(
        js_string!("removeEventListener"),
        2,
        NativeFunction::from_fn_ptr(window_remove_listener),
    )?;
    context.register_global_callable(
        js_string!("fetch"),
        1,
        NativeFunction::from_fn_ptr(fetch_resource),
    )?;
    let local_storage = storage_object(context, false)?;
    let session_storage = storage_object(context, true)?;
    context.register_global_property(js_string!("localStorage"), local_storage, flags)?;
    context.register_global_property(js_string!("sessionStorage"), session_storage, flags)?;
    crate::web::register(context)?;
    context.register_global_callable(
        js_string!("requestIdleCallback"),
        1,
        NativeFunction::from_fn_ptr(request_idle),
    )?;
    context.register_global_callable(
        js_string!("cancelIdleCallback"),
        1,
        NativeFunction::from_fn_ptr(clear_timer),
    )?;
    context.register_global_callable(
        js_string!("matchMedia"),
        1,
        NativeFunction::from_fn_ptr(match_media),
    )?;
    context.register_global_callable(
        js_string!("getComputedStyle"),
        1,
        NativeFunction::from_fn_ptr(get_computed_style),
    )?;
    context.register_global_callable(
        js_string!("queueMicrotask"),
        1,
        NativeFunction::from_fn_ptr(queue_microtask),
    )?;
    let custom = custom_elements(context)?;
    context.register_global_property(js_string!("customElements"), custom, flags)?;
    if let Ok(script) = Script::parse(Source::from_bytes(DATASET_HELPER), None, context) {
        let _ = script.evaluate(context);
    }
    let _ = wire_document_prototype(context);
    Ok(())
}

fn wire_document_prototype(context: &mut Context) -> JsResult<()> {
    let document = context
        .global_object()
        .get(js_string!("document"), context)?;
    let ctor = context
        .global_object()
        .get(js_string!("Document"), context)?;
    let (Some(document), Some(ctor)) = (document.as_object(), ctor.as_object()) else {
        return Ok(());
    };
    let proto = ctor.get(js_string!("prototype"), context)?;
    if let Some(proto) = proto.as_object() {
        document.set_prototype(Some(proto.clone()));
    }
    Ok(())
}

const DATASET_HELPER: &str = r#"
function __mithuDataset(id) {
  return new Proxy({}, {
    get: function (_target, key) {
      if (typeof key !== "string") return undefined;
      return document.__dataGet(id, key);
    },
    set: function (_target, key, value) {
      document.__dataSet(id, String(key), value == null ? "" : String(value));
      return true;
    }
  });
}
"#;

fn deliver_mutations(context: &mut Context) -> JsResult<()> {
    crate::web::deliver_mutations(context)
}

fn storage_object(context: &mut Context, session: bool) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    if session {
        object.function(
            NativeFunction::from_fn_ptr(session_get),
            js_string!("getItem"),
            1,
        );
        object.function(
            NativeFunction::from_fn_ptr(session_set),
            js_string!("setItem"),
            2,
        );
        object.function(
            NativeFunction::from_fn_ptr(session_remove),
            js_string!("removeItem"),
            1,
        );
        object.function(
            NativeFunction::from_fn_ptr(session_clear),
            js_string!("clear"),
            0,
        );
    } else {
        object.function(
            NativeFunction::from_fn_ptr(local_get),
            js_string!("getItem"),
            1,
        );
        object.function(
            NativeFunction::from_fn_ptr(local_set),
            js_string!("setItem"),
            2,
        );
        object.function(
            NativeFunction::from_fn_ptr(local_remove),
            js_string!("removeItem"),
            1,
        );
        object.function(
            NativeFunction::from_fn_ptr(local_clear),
            js_string!("clear"),
            0,
        );
    }
    Ok(object.build())
}

fn storage_bag(
    context: &mut Context,
    session: bool,
) -> JsResult<Rc<RefCell<HashMap<String, String>>>> {
    let host = host(context)?;
    Ok(if session {
        host.session_store
    } else {
        host.local
    })
}

fn local_get(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_get(false, args, context)
}

fn session_get(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_get(true, args, context)
}

fn storage_get(session: bool, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = arg_string(args, 0, context)?;
    let bag = storage_bag(context, session)?;
    Ok(match bag.borrow().get(&key) {
        Some(value) => js_string!(value.clone()).into(),
        None => JsValue::null(),
    })
}

fn local_set(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_set(false, args, context)
}

fn session_set(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_set(true, args, context)
}

fn storage_set(session: bool, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    storage_bag(context, session)?
        .borrow_mut()
        .insert(key, value);
    Ok(JsValue::undefined())
}

fn local_remove(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_remove(false, args, context)
}

fn session_remove(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_remove(true, args, context)
}

fn storage_remove(session: bool, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = arg_string(args, 0, context)?;
    storage_bag(context, session)?.borrow_mut().remove(&key);
    Ok(JsValue::undefined())
}

fn local_clear(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_bag(context, false)?.borrow_mut().clear();
    Ok(JsValue::undefined())
}

fn session_clear(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    storage_bag(context, true)?.borrow_mut().clear();
    Ok(JsValue::undefined())
}

fn console_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object.function(
        NativeFunction::from_fn_ptr(console_log),
        js_string!("log"),
        1,
    );
    object.function(
        NativeFunction::from_fn_ptr(console_log),
        js_string!("warn"),
        1,
    );
    object.function(
        NativeFunction::from_fn_ptr(console_log),
        js_string!("error"),
        1,
    );
    Ok(object.build())
}

fn navigator_object(context: &mut Context) -> JsResult<JsObject> {
    let languages = JsArray::from_iter([JsValue::from(js_string!("en"))], context);
    let mut object = ObjectInitializer::new(context);
    object.property(js_string!("userAgent"), js_string!("Mithu"), ATTR);
    object.property(js_string!("platform"), js_string!("Linux x86_64"), ATTR);
    object.property(js_string!("language"), js_string!("en"), ATTR);
    object.property(js_string!("languages"), languages, ATTR);
    object.property(js_string!("doNotTrack"), JsValue::null(), ATTR);
    object.property(js_string!("cookieEnabled"), JsValue::from(true), ATTR);
    object.property(js_string!("onLine"), JsValue::from(true), ATTR);
    object.function(
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(true))),
        js_string!("sendBeacon"),
        1,
    );
    Ok(object.build())
}

fn location_object(context: &mut Context) -> JsResult<JsObject> {
    let href_get = native(context, "get href", 0, get_location);
    let href_set = native(context, "set href", 1, set_location);
    let protocol_get = native(context, "get protocol", 0, get_protocol);
    let host_get = native(context, "get host", 0, get_host_part);
    let hostname_get = native(context, "get hostname", 0, get_hostname);
    let port_get = native(context, "get port", 0, get_port);
    let path_get = native(context, "get pathname", 0, get_pathname);
    let search_get = native(context, "get search", 0, get_search);
    let hash_get = native(context, "get hash", 0, get_hash);
    let origin_get = native(context, "get origin", 0, get_origin);
    let mut object = ObjectInitializer::new(context);
    object
        .accessor(js_string!("href"), Some(href_get), Some(href_set), ATTR)
        .accessor(js_string!("protocol"), Some(protocol_get), None, ATTR)
        .accessor(js_string!("host"), Some(host_get), None, ATTR)
        .accessor(js_string!("hostname"), Some(hostname_get), None, ATTR)
        .accessor(js_string!("port"), Some(port_get), None, ATTR)
        .accessor(js_string!("pathname"), Some(path_get), None, ATTR)
        .accessor(js_string!("search"), Some(search_get), None, ATTR)
        .accessor(js_string!("hash"), Some(hash_get), None, ATTR)
        .accessor(js_string!("origin"), Some(origin_get), None, ATTR);
    Ok(object.build())
}

fn location_url(context: &mut Context) -> Option<mithu_url::Url> {
    let host = host(context).ok()?;
    mithu_url::Url::parse(&host.location.borrow()).ok()
}

fn format_part(url: &mithu_url::Url, name: &str) -> String {
    match name {
        "protocol" => format!("{}:", url.scheme),
        "host" => match url.port {
            Some(port) => format!("{}:{port}", url.host),
            None => url.host.clone(),
        },
        "hostname" => url.host.clone(),
        "port" => url.port.map(|port| port.to_string()).unwrap_or_default(),
        "pathname" => {
            if url.path.is_empty() {
                "/".to_string()
            } else {
                url.path.clone()
            }
        }
        "search" => url
            .query
            .as_ref()
            .map(|query| format!("?{query}"))
            .unwrap_or_default(),
        "hash" => url
            .fragment
            .as_ref()
            .map(|fragment| format!("#{fragment}"))
            .unwrap_or_default(),
        "origin" => {
            let port = url.port.map(|port| format!(":{port}")).unwrap_or_default();
            format!("{}://{}{port}", url.scheme, url.host)
        }
        _ => String::new(),
    }
}

fn get_protocol(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "protocol"))
            .unwrap_or_default()
    )
    .into())
}

fn get_host_part(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "host"))
            .unwrap_or_default()
    )
    .into())
}

fn get_hostname(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "hostname"))
            .unwrap_or_default()
    )
    .into())
}

fn get_port(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "port"))
            .unwrap_or_default()
    )
    .into())
}

fn get_pathname(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "pathname"))
            .unwrap_or_default()
    )
    .into())
}

fn get_search(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "search"))
            .unwrap_or_default()
    )
    .into())
}

fn get_hash(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "hash"))
            .unwrap_or_default()
    )
    .into())
}

fn get_origin(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(
        location_url(context)
            .map(|url| format_part(&url, "origin"))
            .unwrap_or_default()
    )
    .into())
}

fn document_object(context: &mut Context) -> JsResult<JsObject> {
    let title_get = native(context, "get title", 0, get_title);
    let title_set = native(context, "set title", 1, set_title);
    let body_get = native(context, "get body", 0, get_body);
    let document_get = native(context, "get documentElement", 0, get_document_element);
    let head_get = native(context, "get head", 0, get_head);
    let script_get = native(context, "get currentScript", 0, get_current_script);
    let sheets_get = native(context, "get styleSheets", 0, get_style_sheets);
    let ready_get = native(context, "get readyState", 0, get_ready_state);
    let base_get = native(context, "get baseURI", 0, get_location);
    let cookie_get = native(context, "get cookie", 0, get_cookie);
    let cookie_set = native(context, "set cookie", 1, set_cookie);
    let window = context.global_object().clone();
    let mut object = ObjectInitializer::new(context);
    object
        .function(
            NativeFunction::from_fn_ptr(get_element_by_id),
            js_string!("getElementById"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(query_selector),
            js_string!("querySelector"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(query_selector_all),
            js_string!("querySelectorAll"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_element),
            js_string!("createElement"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_text_node),
            js_string!("createTextNode"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(get_elements_by_tag_name),
            js_string!("getElementsByTagName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(get_elements_by_class_name),
            js_string!("getElementsByClassName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(document_listener),
            js_string!("addEventListener"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(document_remove_listener),
            js_string!("removeEventListener"),
            2,
        )
        .accessor(js_string!("title"), Some(title_get), Some(title_set), ATTR)
        .accessor(js_string!("body"), Some(body_get), None, ATTR)
        .accessor(
            js_string!("documentElement"),
            Some(document_get),
            None,
            ATTR,
        )
        .accessor(js_string!("head"), Some(head_get), None, ATTR)
        .accessor(js_string!("currentScript"), Some(script_get), None, ATTR)
        .accessor(js_string!("styleSheets"), Some(sheets_get), None, ATTR)
        .accessor(js_string!("readyState"), Some(ready_get), None, ATTR)
        .accessor(js_string!("baseURI"), Some(base_get), None, ATTR)
        .accessor(
            js_string!("cookie"),
            Some(cookie_get),
            Some(cookie_set),
            ATTR,
        )
        .property(js_string!("referrer"), js_string!(""), ATTR)
        .property(js_string!("visibilityState"), js_string!("visible"), ATTR)
        .property(js_string!("hidden"), JsValue::from(false), ATTR)
        .property(js_string!("prerendering"), JsValue::from(false), ATTR)
        .property(js_string!("compatMode"), js_string!("CSS1Compat"), ATTR)
        .property(js_string!("nodeType"), JsValue::from(9), ATTR)
        .property(js_string!("defaultView"), window, ATTR)
        .function(
            NativeFunction::from_fn_ptr(create_element_ns),
            js_string!("createElementNS"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(data_get),
            js_string!("__dataGet"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(data_set),
            js_string!("__dataSet"),
            3,
        );
    Ok(object.build())
}

fn data_attribute(key: &str) -> String {
    let mut name = String::from("data-");
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            name.push('-');
            name.push(ch.to_ascii_lowercase());
        } else {
            name.push(ch);
        }
    }
    name
}

fn data_get(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = args
        .first()
        .and_then(|value| value.as_number())
        .unwrap_or(0.0) as usize;
    let key = arg_string(args, 1, context)?;
    match host.doc.borrow().attr(NodeId(id), &data_attribute(&key)) {
        Some(value) => Ok(js_string!(value.to_string()).into()),
        None => Ok(JsValue::null()),
    }
}

fn data_set(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = args
        .first()
        .and_then(|value| value.as_number())
        .unwrap_or(0.0) as usize;
    let key = arg_string(args, 1, context)?;
    let value = arg_string(args, 2, context)?;
    host.doc
        .borrow_mut()
        .set_attr(NodeId(id), &data_attribute(&key), &value);
    note_mutation(
        context,
        NodeId(id),
        "attributes",
        Some(data_attribute(&key)),
    )?;
    Ok(JsValue::undefined())
}

fn get_style_sheets(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let ids = {
        let host = host(context)?;
        let doc = host.doc.borrow();
        let mut found = Vec::new();
        fn walk(doc: &Document, id: NodeId, found: &mut Vec<NodeId>) {
            let style = doc.element_name(id) == Some("style");
            let link = doc.element_name(id) == Some("link")
                && doc
                    .attr(id, "rel")
                    .is_some_and(|rel| rel.eq_ignore_ascii_case("stylesheet"));
            if style || link {
                found.push(id);
            }
            for child in doc.node(id).children.clone() {
                walk(doc, child, found);
            }
        }
        walk(&doc, doc.root, &mut found);
        found
    };
    let mut values = Vec::new();
    for id in ids {
        let element = element_value(id.0, context);
        values.push(sheet_for(&element, id, context)?);
    }
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn get_sheet(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    sheet_for(this, id, context)
}

fn sheet_for(element: &JsValue, id: NodeId, context: &mut Context) -> JsResult<JsValue> {
    if let Some(object) = element.as_object() {
        let cached = object.get(js_string!("__sheet"), context)?;
        if cached.is_object() {
            return Ok(cached);
        }
        let rules = JsObject::from(JsArray::from_iter(Vec::<JsValue>::new(), context));
        let mut sheet = ObjectInitializer::new(context);
        sheet
            .property(js_string!("ownerNode"), element.clone(), ATTR)
            .property(js_string!("__node"), JsValue::from(id.0 as u32), ATTR)
            .property(js_string!("cssRules"), rules, ATTR)
            .function(
                NativeFunction::from_fn_ptr(sheet_insert_rule),
                js_string!("insertRule"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                js_string!("deleteRule"),
                1,
            );
        let sheet = JsValue::from(sheet.build());
        object.set(js_string!("__sheet"), sheet.clone(), true, context)?;
        return Ok(sheet);
    }
    Ok(JsValue::undefined())
}

fn sheet_insert_rule(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = this
        .as_object()
        .and_then(|object| object.get(js_string!("__node"), context).ok())
        .and_then(|value| value.as_number())
        .unwrap_or(0.0) as usize;
    let rule = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    let next = format!("{}{}", doc.text_content(NodeId(id)), rule);
    doc.set_text_content(NodeId(id), &next);
    let index = args
        .get(1)
        .and_then(|value| value.as_number())
        .unwrap_or(0.0);
    Ok(JsValue::from(index))
}

fn get_current_script(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = {
        let host = host(context)?;
        *host.current_script.borrow()
    };
    Ok(match id {
        Some(id) => element_value(id.0, context),
        None => JsValue::null(),
    })
}

fn get_cookie(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    // HttpOnly cookies live in the network jar and are not visible to scripts.
    // The string is empty until the jar can tell those apart.
    Ok(js_string!("").into())
}

fn set_cookie(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn get_ready_state(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(js_string!(host.ready_state.borrow().clone()).into())
}

fn signal_document_loaded(context: &mut Context) {
    let Some(html) = (|| {
        let host = context.get_data::<Host>().cloned()?;
        *host.ready_state.borrow_mut() = "complete".to_string();
        find_tag(&host.doc.borrow(), "html")
    })() else {
        return;
    };
    fire_node_listeners(context, html, "DOMContentLoaded");
    fire_window_listeners(context, html, "DOMContentLoaded");
    fire_window_listeners(context, html, "load");
}

fn fire_node_listeners(context: &mut Context, id: NodeId, event: &str) {
    let functions = listener_functions(context, id, event).unwrap_or_default();
    if functions.is_empty() {
        return;
    }
    if let Some(host) = context.get_data::<Host>().cloned() {
        host.event_target.set(Some(id.0));
        host.event_current.set(Some(id.0));
    }
    let Ok(event_value) = event_object(id, event, context) else {
        return;
    };
    let this = element_value(id.0, context);
    for function in functions {
        let _ = function.call(&this, &[event_value.clone()], context);
    }
}

fn fire_window_listeners(context: &mut Context, target: NodeId, event: &str) {
    let functions = read_listeners(context, &format!("win:{event}")).unwrap_or_default();
    if functions.is_empty() {
        return;
    }
    let Ok(event_value) = event_object(target, event, context) else {
        return;
    };
    let this = JsValue::from(context.global_object().clone());
    for function in functions {
        let _ = function.call(&this, &[event_value.clone()], context);
    }
}

pub(crate) fn native(
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
        let mut text = arg.to_string(context)?.to_std_string_lossy();
        if let Some(object) = arg.as_object()
            && let Ok(stack) = object.get(js_string!("stack"), context)
        {
            let stack = stack.to_string(context)?.to_std_string_lossy();
            if !stack.is_empty() && stack != "undefined" && !text.contains(&stack) {
                text.push('\n');
                text.push_str(&stack);
            }
        }
        parts.push(text);
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

fn get_element_by_id(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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

fn query_selector_all(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let scope = node_of(this).unwrap_or(NodeId(0));
    let ids = {
        let doc = host.doc.borrow();
        query_all(&doc, scope, &selector, node_of(this).is_some())
    };
    let values = ids
        .into_iter()
        .map(|id| element_value(id.0, context))
        .collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn get_elements_by_tag_name(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?.to_ascii_lowercase();
    collect_named(this, context, |doc, id| {
        doc.element_name(id)
            .is_some_and(|element| name == "*" || element == name)
    })
}

fn get_elements_by_class_name(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
    let values = ids
        .into_iter()
        .map(|id| element_value(id.0, context))
        .collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn collect_matching(
    doc: &Document,
    id: NodeId,
    keep: &mut impl FnMut(&Document, NodeId) -> bool,
    out: &mut Vec<NodeId>,
) {
    if keep(doc, id) {
        out.push(id);
    }
    for child in doc.node(id).children.clone() {
        collect_matching(doc, child, keep, out);
    }
}

fn create_element_ns(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| args.first().cloned().unwrap_or(JsValue::undefined()));
    create_element(this, &[name], context)
}

fn create_element(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let name = arg_string(args, 0, context)?;
    let id = host.doc.borrow_mut().create_element_detached(&name);
    Ok(element_value(id.0, context))
}

fn cached_node(id: usize, context: &mut Context) -> Option<JsValue> {
    let bag = context
        .global_object()
        .get(js_string!("__mithuNodes"), context)
        .ok()?;
    let bag = bag.as_object()?;
    let value = bag.get(id as u32, context).ok()?;
    value.is_object().then_some(value)
}

fn remember_node(id: usize, value: &JsValue, context: &mut Context) {
    let Ok(bag) = context
        .global_object()
        .get(js_string!("__mithuNodes"), context)
    else {
        return;
    };
    let Some(bag) = bag.as_object() else {
        return;
    };
    let _ = bag.set(id as u32, value.clone(), true, context);
}

pub(crate) fn element_value(id: usize, context: &mut Context) -> JsValue {
    if let Some(cached) = cached_node(id, context) {
        return cached;
    }
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
    let previous_get = native(
        context,
        "get previousElementSibling",
        0,
        get_previous_sibling,
    );
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
    let lang_get = native(context, "get lang", 0, get_lang);
    let lang_set = native(context, "set lang", 1, set_lang);
    let reflect_src = context.get_data::<Host>().is_some_and(|host| {
        matches!(
            host.doc.borrow().element_name(NodeId(id)).unwrap_or(""),
            "script" | "img" | "iframe" | "source" | "audio" | "video" | "embed" | "track"
        )
    });
    let src_get = reflect_src.then(|| native(context, "get src", 0, get_src));
    let src_set = reflect_src.then(|| native(context, "set src", 1, set_src));
    let dataset_get = native(context, "get dataset", 0, get_dataset);
    let owner_get = native(context, "get ownerDocument", 0, get_owner_document);
    let offset_width = native(context, "get offsetWidth", 0, get_offset_width);
    let offset_height = native(context, "get offsetHeight", 0, get_offset_height);
    let client_width = native(context, "get clientWidth", 0, get_client_width);
    let client_height = native(context, "get clientHeight", 0, get_client_height);
    let offset_left = native(context, "get offsetLeft", 0, get_offset_left);
    let offset_top = native(context, "get offsetTop", 0, get_offset_top);
    let scroll_width = native(context, "get scrollWidth", 0, get_offset_width);
    let scroll_height = native(context, "get scrollHeight", 0, get_offset_height);
    let anchor = context.get_data::<Host>().is_some_and(|host| {
        matches!(
            host.doc.borrow().element_name(NodeId(id)).unwrap_or(""),
            "a" | "area"
        )
    });
    let anchor_host = native(context, "get anchor host", 0, get_anchor_host);
    let anchor_hostname = native(context, "get anchor hostname", 0, get_anchor_hostname);
    let anchor_path = native(context, "get anchor pathname", 0, get_anchor_pathname);
    let anchor_protocol = native(context, "get anchor protocol", 0, get_anchor_protocol);
    let anchor_search = native(context, "get anchor search", 0, get_anchor_search);
    let anchor_hash = native(context, "get anchor hash", 0, get_anchor_hash);
    let has_sheet = context.get_data::<Host>().is_some_and(|host| {
        matches!(
            host.doc.borrow().element_name(NodeId(id)).unwrap_or(""),
            "style" | "link"
        )
    });
    let sheet_get = has_sheet.then(|| native(context, "get sheet", 0, get_sheet));
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .function(
            NativeFunction::from_fn_ptr(get_attribute),
            js_string!("getAttribute"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(set_attribute),
            js_string!("setAttribute"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(remove_attribute),
            js_string!("removeAttribute"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(has_attribute),
            js_string!("hasAttribute"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(query_selector),
            js_string!("querySelector"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(query_selector_all),
            js_string!("querySelectorAll"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(get_elements_by_tag_name),
            js_string!("getElementsByTagName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(get_elements_by_class_name),
            js_string!("getElementsByClassName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(matches_selector),
            js_string!("matches"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(closest_selector),
            js_string!("closest"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(contains_node),
            js_string!("contains"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(dispatch_event),
            js_string!("dispatchEvent"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(append_child),
            js_string!("appendChild"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(append_nodes),
            js_string!("append"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(prepend_nodes),
            js_string!("prepend"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(insert_adjacent_html),
            js_string!("insertAdjacentHTML"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(insert_before_child),
            js_string!("insertBefore"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(remove_child),
            js_string!("removeChild"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(add_event_listener),
            js_string!("addEventListener"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(remove_event_listener),
            js_string!("removeEventListener"),
            2,
        )
        .accessor(js_string!("onclick"), None, Some(click_set), ATTR)
        .accessor(js_string!("oninput"), None, Some(input_set), ATTR)
        .accessor(js_string!("onchange"), None, Some(change_set), ATTR)
        .accessor(js_string!("onsubmit"), None, Some(submit_set), ATTR)
        .accessor(
            js_string!("textContent"),
            Some(text_get),
            Some(text_set),
            ATTR,
        )
        .accessor(
            js_string!("innerText"),
            Some(inner_get),
            Some(inner_set),
            ATTR,
        )
        .accessor(
            js_string!("innerHTML"),
            Some(html_get),
            Some(html_set),
            ATTR,
        )
        .accessor(js_string!("id"), Some(id_get), Some(id_set), ATTR)
        .accessor(
            js_string!("className"),
            Some(class_get),
            Some(class_set),
            ATTR,
        )
        .accessor(js_string!("parentElement"), Some(parent_get), None, ATTR)
        .accessor(js_string!("nextElementSibling"), Some(next_get), None, ATTR)
        .accessor(
            js_string!("previousElementSibling"),
            Some(previous_get),
            None,
            ATTR,
        )
        .accessor(js_string!("children"), Some(children_get), None, ATTR)
        .accessor(js_string!("childNodes"), Some(nodes_get), None, ATTR)
        .accessor(js_string!("firstChild"), Some(first_get), None, ATTR)
        .accessor(js_string!("lastChild"), Some(last_get), None, ATTR)
        .accessor(js_string!("nextSibling"), Some(next_node_get), None, ATTR)
        .accessor(
            js_string!("previousSibling"),
            Some(previous_node_get),
            None,
            ATTR,
        )
        .accessor(js_string!("parentNode"), Some(parent_node_get), None, ATTR)
        .accessor(js_string!("nodeType"), Some(type_get), None, ATTR)
        .accessor(js_string!("nodeName"), Some(name_get.clone()), None, ATTR)
        .accessor(js_string!("tagName"), Some(name_get), None, ATTR)
        .accessor(js_string!("value"), Some(value_get), Some(value_set), ATTR)
        .accessor(
            js_string!("checked"),
            Some(checked_get),
            Some(checked_set),
            ATTR,
        )
        .accessor(js_string!("href"), Some(href_get), Some(href_set), ATTR)
        .accessor(js_string!("lang"), Some(lang_get), Some(lang_set), ATTR);
    if let (Some(src_get), Some(src_set)) = (src_get, src_set) {
        object.accessor(js_string!("src"), Some(src_get), Some(src_set), ATTR);
    }
    object
        .property(js_string!("classList"), class_list, ATTR)
        .property(js_string!("style"), style, ATTR)
        .property(js_string!("nodeType"), JsValue::from(1), ATTR)
        .function(
            NativeFunction::from_fn_ptr(get_bounding_rect),
            js_string!("getBoundingClientRect"),
            0,
        )
        .accessor(js_string!("offsetWidth"), Some(offset_width), None, ATTR)
        .accessor(js_string!("offsetHeight"), Some(offset_height), None, ATTR)
        .accessor(js_string!("clientWidth"), Some(client_width), None, ATTR)
        .accessor(js_string!("clientHeight"), Some(client_height), None, ATTR)
        .accessor(js_string!("offsetLeft"), Some(offset_left), None, ATTR)
        .accessor(js_string!("offsetTop"), Some(offset_top), None, ATTR)
        .accessor(js_string!("scrollWidth"), Some(scroll_width), None, ATTR)
        .accessor(js_string!("scrollHeight"), Some(scroll_height), None, ATTR);
    object
        .accessor(js_string!("dataset"), Some(dataset_get), None, ATTR)
        .accessor(js_string!("ownerDocument"), Some(owner_get), None, ATTR);
    if anchor {
        object
            .accessor(js_string!("host"), Some(anchor_host), None, ATTR)
            .accessor(js_string!("hostname"), Some(anchor_hostname), None, ATTR)
            .accessor(js_string!("pathname"), Some(anchor_path), None, ATTR)
            .accessor(js_string!("protocol"), Some(anchor_protocol), None, ATTR)
            .accessor(js_string!("search"), Some(anchor_search), None, ATTR)
            .accessor(js_string!("hash"), Some(anchor_hash), None, ATTR);
    }
    if let Some(sheet_get) = sheet_get {
        object.accessor(js_string!("sheet"), Some(sheet_get), None, ATTR);
    }
    let value = object.build();
    if let Ok(ctor) = context
        .global_object()
        .get(js_string!("HTMLElement"), context)
        && let Some(ctor) = ctor.as_object()
        && let Ok(proto) = ctor.get(js_string!("prototype"), context)
        && let Some(proto) = proto.as_object()
    {
        value.set_prototype(Some(proto.clone()));
    }
    let value = JsValue::from(value);
    remember_node(id, &value, context);
    value
}

fn get_dataset(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    let global = context.global_object();
    let factory = global.get(js_string!("__mithuDataset"), context)?;
    let Some(factory) = factory.as_object().filter(|object| object.is_callable()) else {
        return Ok(JsObject::from(JsArray::from_iter([], context)).into());
    };
    factory.call(
        &JsValue::undefined(),
        &[JsValue::from(id.0 as u32)],
        context,
    )
}

fn get_owner_document(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    context.global_object().get(js_string!("document"), context)
}

fn box_metrics(this: &JsValue, context: &mut Context) -> Option<BoxMetrics> {
    let host = host(context).ok()?;
    let id = node_of(this)?;
    host.geometry.borrow().get(&id.0).copied()
}

fn get_bounding_rect(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let metrics = box_metrics(this, context).unwrap_or_default();
    let scroll = host(context).map(|host| host.scroll.get()).unwrap_or(0.0);
    let x = f64::from(metrics.x);
    let y = f64::from(metrics.y - scroll);
    let width = f64::from(metrics.width);
    let height = f64::from(metrics.height);
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("x"), JsValue::from(x), ATTR)
        .property(js_string!("y"), JsValue::from(y), ATTR)
        .property(js_string!("width"), JsValue::from(width), ATTR)
        .property(js_string!("height"), JsValue::from(height), ATTR)
        .property(js_string!("left"), JsValue::from(x), ATTR)
        .property(js_string!("top"), JsValue::from(y), ATTR)
        .property(js_string!("right"), JsValue::from(x + width), ATTR)
        .property(js_string!("bottom"), JsValue::from(y + height), ATTR);
    Ok(object.build().into())
}

fn metric_number(
    this: &JsValue,
    context: &mut Context,
    pick: fn(BoxMetrics) -> f32,
) -> JsResult<JsValue> {
    let metrics = box_metrics(this, context).unwrap_or_default();
    Ok(JsValue::from(f64::from(pick(metrics))))
}

fn get_offset_width(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| metrics.width)
}

fn get_offset_height(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| metrics.height)
}

fn get_client_width(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| {
        (metrics.width - metrics.border_left - metrics.border_right).max(0.0)
    })
}

fn get_client_height(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| {
        (metrics.height - metrics.border_top - metrics.border_bottom).max(0.0)
    })
}

fn get_offset_left(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| metrics.x)
}

fn get_offset_top(this: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    metric_number(this, context, |metrics| metrics.y)
}

fn anchor_url(this: &JsValue, context: &mut Context) -> Option<mithu_url::Url> {
    let host = host(context).ok()?;
    let id = node_of(this)?;
    let raw = host.doc.borrow().attr(id, "href").unwrap_or("").to_string();
    let base = host.location.borrow().clone();
    mithu_url::Url::parse(&base)
        .ok()
        .and_then(|base| base.resolve(&raw).ok())
        .or_else(|| mithu_url::Url::parse(&raw).ok())
}

fn anchor_part(this: &JsValue, context: &mut Context, name: &str) -> JsResult<JsValue> {
    Ok(js_string!(
        anchor_url(this, context)
            .map(|url| format_part(&url, name))
            .unwrap_or_default()
    )
    .into())
}

fn get_anchor_host(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    anchor_part(this, context, "host")
}

fn get_anchor_hostname(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    anchor_part(this, context, "hostname")
}

fn get_anchor_pathname(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    anchor_part(this, context, "pathname")
}

fn get_anchor_protocol(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    anchor_part(this, context, "protocol")
}

fn get_anchor_search(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    anchor_part(this, context, "search")
}

fn get_anchor_hash(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    anchor_part(this, context, "hash")
}

fn note_mutation(
    context: &mut Context,
    target: NodeId,
    kind: &str,
    attribute: Option<String>,
) -> JsResult<()> {
    let host = host(context)?;
    host.doc
        .borrow_mut()
        .mutations
        .push(mithu_dom::DomMutation {
            target,
            kind: kind.to_string(),
            attribute,
        });
    Ok(())
}

fn match_media(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = arg_string(args, 0, context).unwrap_or_default();
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("media"), js_string!(query), ATTR)
        .property(js_string!("matches"), JsValue::from(false), ATTR)
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("addListener"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("removeListener"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("addEventListener"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("removeEventListener"),
            2,
        );
    Ok(object.build().into())
}

fn get_computed_style(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().and_then(node_of).ok_or_else(type_error)?;
    let mut object = ObjectInitializer::with_native_data(NodeKey { id: id.0 }, context);
    object.function(
        NativeFunction::from_fn_ptr(computed_property),
        js_string!("getPropertyValue"),
        1,
    );
    Ok(object.build().into())
}

fn computed_property(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let Some(object) = this.as_object() else {
        return Ok(js_string!("").into());
    };
    let Some(key) = object.downcast_ref::<NodeKey>() else {
        return Ok(js_string!("").into());
    };
    let name = arg_string(args, 0, context)?;
    let style = host
        .doc
        .borrow()
        .attr(NodeId(key.id), "style")
        .unwrap_or("")
        .to_string();
    let value = parse_declarations(&style)
        .into_iter()
        .find(|declaration| declaration.name.eq_ignore_ascii_case(&name))
        .map(|declaration| declaration.value)
        .unwrap_or_default();
    Ok(js_string!(value).into())
}

fn queue_microtask(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if let Some(function) = args
        .first()
        .and_then(|value| value.as_object())
        .filter(|object| object.is_callable())
    {
        let _ = function.call(&JsValue::undefined(), &[], context);
    }
    Ok(JsValue::undefined())
}

fn custom_elements(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("define"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("get"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, context| {
                Ok(JsPromise::resolve(JsValue::undefined(), context)?.into())
            }),
            js_string!("whenDefined"),
            1,
        );
    Ok(object.build())
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
    let right_get = native(context, "get right", 0, get_style_right);
    let right_set = native(context, "set right", 1, set_style_right);
    let bottom_get = native(context, "get bottom", 0, get_style_bottom);
    let bottom_set = native(context, "set bottom", 1, set_style_bottom);
    let visibility_get = native(context, "get visibility", 0, get_style_visibility);
    let visibility_set = native(context, "set visibility", 1, set_style_visibility);
    let border_get = native(context, "get border", 0, get_style_border);
    let border_set = native(context, "set border", 1, set_style_border);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("cssText"), Some(css_get), Some(css_set), ATTR)
        .function(
            NativeFunction::from_fn_ptr(set_property),
            js_string!("setProperty"),
            2,
        )
        .accessor(js_string!("color"), Some(color_get), Some(color_set), ATTR)
        .accessor(
            js_string!("backgroundColor"),
            Some(background_get),
            Some(background_set),
            ATTR,
        )
        .accessor(
            js_string!("display"),
            Some(display_get),
            Some(display_set),
            ATTR,
        )
        .accessor(js_string!("width"), Some(width_get), Some(width_set), ATTR)
        .accessor(
            js_string!("height"),
            Some(height_get),
            Some(height_set),
            ATTR,
        )
        .accessor(js_string!("fontSize"), Some(font_get), Some(font_set), ATTR)
        .accessor(
            js_string!("margin"),
            Some(margin_get),
            Some(margin_set),
            ATTR,
        )
        .accessor(
            js_string!("padding"),
            Some(padding_get),
            Some(padding_set),
            ATTR,
        )
        .accessor(
            js_string!("textAlign"),
            Some(align_get),
            Some(align_set),
            ATTR,
        )
        .accessor(
            js_string!("position"),
            Some(position_get),
            Some(position_set),
            ATTR,
        )
        .accessor(js_string!("top"), Some(top_get), Some(top_set), ATTR)
        .accessor(js_string!("left"), Some(left_get), Some(left_set), ATTR)
        .accessor(
            js_string!("opacity"),
            Some(opacity_get),
            Some(opacity_set),
            ATTR,
        )
        .accessor(
            js_string!("flexDirection"),
            Some(flex_get),
            Some(flex_set),
            ATTR,
        )
        .accessor(js_string!("gap"), Some(gap_get), Some(gap_set), ATTR)
        .accessor(js_string!("right"), Some(right_get), Some(right_set), ATTR)
        .accessor(
            js_string!("bottom"),
            Some(bottom_get),
            Some(bottom_set),
            ATTR,
        )
        .accessor(
            js_string!("visibility"),
            Some(visibility_get),
            Some(visibility_set),
            ATTR,
        )
        .accessor(
            js_string!("border"),
            Some(border_get),
            Some(border_set),
            ATTR,
        );
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
style_fns!(
    get_style_background,
    set_style_background,
    "background-color"
);
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
style_fns!(
    get_style_flex_direction,
    set_style_flex_direction,
    "flex-direction"
);
style_fns!(get_style_gap, set_style_gap, "gap");
style_fns!(get_style_right, set_style_right, "right");
style_fns!(get_style_bottom, set_style_bottom, "bottom");
style_fns!(get_style_visibility, set_style_visibility, "visibility");
style_fns!(get_style_border, set_style_border, "border");

fn get_css_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let text = host
        .doc
        .borrow()
        .attr(id, "style")
        .unwrap_or("")
        .to_string();
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
    let text = host
        .doc
        .borrow()
        .attr(id, "style")
        .unwrap_or("")
        .to_string();
    let value = parse_declarations(&text)
        .into_iter()
        .rev()
        .find(|declaration| declaration.name == name)
        .map(|declaration| declaration.value)
        .unwrap_or_default();
    Ok(js_string!(value).into())
}

fn write_style(
    this: &JsValue,
    name: &str,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    let current = doc.attr(id, "style").unwrap_or("").to_string();
    let mut declarations = parse_declarations(&current);
    if let Some(existing) = declarations
        .iter_mut()
        .find(|declaration| declaration.name == name)
    {
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
    Ok(js_string!(
        host.doc
            .borrow()
            .attr(id, "class")
            .unwrap_or("")
            .to_string()
    )
    .into())
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
    note_mutation(context, id, "attributes", Some(name))?;
    Ok(JsValue::undefined())
}

fn class_list_object(id: usize, context: &mut Context) -> JsObject {
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .function(
            NativeFunction::from_fn_ptr(class_list_add),
            js_string!("add"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(class_list_remove),
            js_string!("remove"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(class_list_toggle),
            js_string!("toggle"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(class_list_contains),
            js_string!("contains"),
            1,
        );
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

fn class_list_contains(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
        if next {
            doc.next_element_sibling(id)
        } else {
            doc.previous_element_sibling(id)
        }
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

fn get_previous_sibling(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
    let values = ids
        .into_iter()
        .map(|child| element_value(child.0, context))
        .collect::<Vec<_>>();
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

fn get_lang(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, "lang").unwrap_or("").to_string()).into())
}

fn set_lang(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "lang", &value);
    Ok(JsValue::undefined())
}

fn append_nodes(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    insert_mixed(this, args, context, false)
}

fn prepend_nodes(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    insert_mixed(this, args, context, true)
}

fn insert_mixed(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
    prepend: bool,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let anchor = if prepend {
        host.doc.borrow().node(parent).children.first().copied()
    } else {
        None
    };
    for arg in args {
        if let Some(child) = node_of(arg) {
            host.doc.borrow_mut().insert_before(parent, child, anchor);
        } else if !arg.is_null_or_undefined() {
            let text = arg.to_string(context)?.to_std_string_lossy();
            if text.is_empty() {
                continue;
            }
            let mut doc = host.doc.borrow_mut();
            let node = doc.create_text_detached(&text);
            doc.insert_before(parent, node, anchor);
        }
    }
    Ok(JsValue::undefined())
}

fn insert_adjacent_html(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let position = arg_string(args, 0, context)?.to_ascii_lowercase();
    let html = arg_string(args, 1, context)?;
    let parsed = parse_html(&format!("<div id=\"mithu-root\">{html}</div>"));
    let Some(root) = parsed.get_element_by_id("mithu-root") else {
        return Ok(this.clone());
    };
    let mut doc = host.doc.borrow_mut();
    let parent = doc.nodes.get(id.0).and_then(|node| node.parent);
    match position.as_str() {
        "beforebegin" => {
            let Some(parent) = parent else {
                return Ok(this.clone());
            };
            graft(&parsed, root, &mut doc, parent, Some(id));
        }
        "afterbegin" => {
            let anchor = doc.node(id).children.first().copied();
            graft(&parsed, root, &mut doc, id, anchor);
        }
        "beforeend" => graft(&parsed, root, &mut doc, id, None),
        "afterend" => {
            let Some(parent) = parent else {
                return Ok(this.clone());
            };
            let next = doc.next_element_sibling(id).or_else(|| {
                let children = &doc.node(parent).children;
                let index = children.iter().position(|child| *child == id)?;
                children.get(index + 1).copied()
            });
            graft(&parsed, root, &mut doc, parent, next);
        }
        _ => {}
    }
    Ok(this.clone())
}

fn graft(
    src: &Document,
    src_parent: NodeId,
    dst: &mut Document,
    dst_parent: NodeId,
    before: Option<NodeId>,
) {
    for child in src.node(src_parent).children.clone() {
        let id = match &src.node(child).data {
            NodeData::Text(text) => dst.create_text_detached(text),
            NodeData::Element(element) => {
                let id = dst.create_element_detached(&element.name);
                for (name, value) in &element.attrs {
                    dst.set_attr(id, name, value);
                }
                copy_children(src, child, dst, id);
                id
            }
            NodeData::Comment(_) | NodeData::Document => continue,
        };
        dst.insert_before(dst_parent, id, before);
    }
}

fn append_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    host.doc.borrow_mut().append_child(parent, child);
    Ok(child_value)
}

fn add_event_listener(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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

fn set_handler(
    this: &JsValue,
    event: &str,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
        .accessor(
            js_string!("defaultPrevented"),
            Some(prevented_get),
            None,
            ATTR,
        )
        .function(
            NativeFunction::from_fn_ptr(prevent_default),
            js_string!("preventDefault"),
            0,
        );
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
    let length = list
        .get(js_string!("length"), context)?
        .as_number()
        .unwrap_or(0.0) as usize;
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
    let length = list
        .get(js_string!("length"), context)?
        .as_number()
        .unwrap_or(0.0);
    list.set(length as u32, function, true, context)?;
    bag.set(js_string!(key), list, true, context)?;
    Ok(())
}

fn run_module(context: &mut Context, code: &str, name: &str) -> JsResult<()> {
    let path = PathBuf::from(name);
    let module = Module::parse(Source::from_bytes(code).with_path(&path), None, context)?;
    // Link only after the loader promise finishes. Linking earlier panics inside Boa
    // when an import has not been written into the module graph yet.
    let promise = module.load_link_evaluate(context);
    context.run_jobs()?;
    match promise.state() {
        boa_engine::builtins::promise::PromiseState::Fulfilled(_) => Ok(()),
        boa_engine::builtins::promise::PromiseState::Rejected(error) => {
            Err(JsError::from_opaque(error))
        }
        boa_engine::builtins::promise::PromiseState::Pending => Err(JsNativeError::typ()
            .with_message("the module did not finish loading")
            .into()),
    }
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
        let (repeat, kind) = {
            let (repeat, kind) = {
                let clock = host.clock.borrow();
                match clock.queue.iter().find(|timer| timer.id == id) {
                    Some(timer) => (timer.repeat, timer.kind),
                    None => (None, 0),
                }
            };
            let mut clock = host.clock.borrow_mut();
            if let Some(every) = repeat {
                let due = clock.now.saturating_add(every);
                if let Some(timer) = clock.queue.iter_mut().find(|timer| timer.id == id) {
                    timer.due = due;
                }
            } else {
                clock.queue.retain(|timer| timer.id != id);
            }
            (repeat, kind)
        };
        let item = timer_value(context, id)?;
        match kind {
            1 => {
                if let Some(function) = item.as_object().filter(|object| object.is_callable()) {
                    let deadline = idle_deadline(context)?;
                    function.call(&JsValue::undefined(), &[deadline], context)?;
                }
            }
            2 => crate::web::finish_xhr(&item, context)?,
            _ => {
                if let Some(function) = item.as_object().filter(|object| object.is_callable()) {
                    function.call(&JsValue::undefined(), &[], context)?;
                }
            }
        }
        if repeat.is_none() {
            forget_timer(context, id)?;
        }
        context.run_jobs()?;
        deliver_mutations(context)?;
    }
    Ok(ran)
}

fn timer_value(context: &mut Context, id: u32) -> JsResult<JsValue> {
    let global = context.global_object();
    let bag = global.get(js_string!("__mithuTimers"), context)?;
    let Some(bag) = bag.as_object() else {
        return Ok(JsValue::undefined());
    };
    bag.get(js_string!(id.to_string()), context)
}

fn idle_deadline(context: &mut Context) -> JsResult<JsValue> {
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("didTimeout"), JsValue::from(false), ATTR)
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(50))),
            js_string!("timeRemaining"),
            0,
        );
    Ok(object.build().into())
}

/// Queue a host callback. `kind` 1 is an idle callback, 2 finishes an async XHR.
pub(crate) fn schedule_callback(
    context: &mut Context,
    value: JsValue,
    kind: u8,
    delay: u64,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = {
        let mut clock = host.clock.borrow_mut();
        clock.next = clock.next.saturating_add(1);
        let id = clock.next;
        let due = clock.now.saturating_add(delay);
        clock.queue.push(Timer {
            id,
            due,
            repeat: None,
            kind,
        });
        id
    };
    let global = context.global_object();
    let bag = global
        .get(js_string!("__mithuTimers"), context)?
        .as_object()
        .ok_or_else(type_error)?;
    bag.set(js_string!(id.to_string()), value, true, context)?;
    Ok(JsValue::from(id))
}

fn request_idle(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let _ = this;
    let function = args.first().cloned().unwrap_or(JsValue::undefined());
    if !function
        .as_object()
        .is_some_and(|object| object.is_callable())
    {
        return Ok(JsValue::from(0));
    }
    let delay = args
        .get(1)
        .and_then(|value| value.as_object())
        .and_then(|object| object.get(js_string!("timeout"), context).ok())
        .and_then(|value| value.as_number())
        .unwrap_or(1.0)
        .max(1.0) as u64;
    schedule_callback(context, function, 1, delay)
}

fn forget_timer(context: &mut Context, id: u32) -> JsResult<()> {
    let global = context.global_object();
    let bag = global.get(js_string!("__mithuTimers"), context)?;
    if let Some(bag) = bag.as_object() {
        bag.set(
            js_string!(id.to_string()),
            JsValue::undefined(),
            true,
            context,
        )?;
    }
    Ok(())
}

fn schedule_timer(context: &mut Context, args: &[JsValue], repeat: bool) -> JsResult<JsValue> {
    let host = host(context)?;
    let function = args.first().cloned().unwrap_or(JsValue::undefined());
    let Some(function) = function.as_object().filter(|object| object.is_callable()) else {
        return Ok(JsValue::from(0));
    };
    let delay = args
        .get(1)
        .and_then(|value| value.as_number())
        .unwrap_or(0.0)
        .max(0.0) as u64;
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
            kind: 0,
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
    let spec = fetch_spec(args, context)?;
    if spec.is_empty() {
        return reject_promise(context, "fetch requires a url");
    }
    let host = host(context)?;
    let location = host.location.borrow().clone();
    let outcome = resolve_request(&location, &spec).and_then(|url| {
        let initiator = mithu_url::Url::parse(&location)
            .ok()
            .map(|page| page.host)
            .filter(|host| !host.is_empty());
        let response = if let Some(host) = initiator.as_deref() {
            mithu_net::get_from(&url, host)
        } else {
            mithu_net::get(&url)
        };
        response
            .map_err(|error| error.to_string())
            .and_then(|response| {
                if let Some(page) = mithu_url::Url::parse(&location).ok()
                    && cross_origin(&page, &response.url)
                    && !cors_allows(&page, response.header("access-control-allow-origin"))
                {
                    return Err("cross-origin fetch was blocked".to_string());
                }
                Ok((
                    response.status,
                    (200..300).contains(&response.status),
                    response.url.to_string(),
                    response.text(),
                    response
                        .headers
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            })
    });
    match outcome {
        Ok((status, ok, url, body, headers)) => {
            let response = response_object(status, ok, url, body, headers, context);
            Ok(JsPromise::resolve(response, context)?.into())
        }
        Err(message) => reject_promise(context, &message),
    }
}

pub(crate) fn cross_origin(page: &mithu_url::Url, target: &mithu_url::Url) -> bool {
    page.scheme != target.scheme || page.host != target.host || page.port != target.port
}

pub(crate) fn cors_allows(page: &mithu_url::Url, allow: Option<&str>) -> bool {
    let Some(allow) = allow.map(str::trim) else {
        return false;
    };
    if allow == "*" {
        return true;
    }
    let port = page.port.map(|port| format!(":{port}")).unwrap_or_default();
    allow == format!("{}://{}{port}", page.scheme, page.host)
}

pub(crate) fn resolve_request(location: &str, spec: &str) -> Result<mithu_url::Url, String> {
    if let Ok(absolute) = mithu_url::Url::parse(spec) {
        return Ok(absolute);
    }
    let base = mithu_url::Url::parse(location).map_err(|error| error.to_string())?;
    base.resolve(spec).map_err(|error| error.to_string())
}

fn reject_promise(context: &mut Context, message: &str) -> JsResult<JsValue> {
    let (promise, resolvers) = JsPromise::new_pending(context);
    resolvers.reject.call(
        &JsValue::undefined(),
        &[js_string!(message).into()],
        context,
    )?;
    Ok(promise.into())
}

fn fetch_spec(args: &[JsValue], context: &mut Context) -> JsResult<String> {
    let Some(value) = args.first() else {
        return Ok(String::new());
    };
    if !value.is_string()
        && let Some(object) = value.as_object()
    {
        let url = object.get(js_string!("url"), context)?;
        if url.is_string() {
            return Ok(url.to_string(context)?.to_std_string_lossy());
        }
    }
    arg_string(args, 0, context)
}

fn response_object(
    status: u16,
    ok: bool,
    url: String,
    body: String,
    headers: String,
    context: &mut Context,
) -> JsValue {
    let headers_object = {
        let mut headers_object = ObjectInitializer::with_native_data(
            ResponseKey {
                body: String::new(),
                headers: headers.clone(),
            },
            context,
        );
        headers_object.function(
            NativeFunction::from_fn_ptr(response_header),
            js_string!("get"),
            1,
        );
        headers_object.build()
    };
    let mut object = ObjectInitializer::with_native_data(ResponseKey { body, headers }, context);
    object
        .property(js_string!("status"), JsValue::from(i32::from(status)), ATTR)
        .property(js_string!("ok"), JsValue::from(ok), ATTR)
        .property(js_string!("url"), js_string!(url), ATTR)
        .property(js_string!("headers"), headers_object, ATTR)
        .function(
            NativeFunction::from_fn_ptr(response_text),
            js_string!("text"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(response_json),
            js_string!("json"),
            0,
        );
    object.build().into()
}

fn response_json(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = response_body(this);
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let Some(json_object) = json.as_object() else {
        return reject_promise(context, "JSON is not available");
    };
    let parse = json_object.get(js_string!("parse"), context)?;
    let Some(parse) = parse.as_object() else {
        return reject_promise(context, "JSON.parse is not available");
    };
    match parse.call(&json, &[js_string!(body).into()], context) {
        Ok(value) => Ok(JsPromise::resolve(value, context)?.into()),
        Err(error) => reject_promise(context, &error.to_string()),
    }
}

fn response_header(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::null());
    };
    let headers = object
        .downcast_ref::<ResponseKey>()
        .map(|key| key.headers.clone())
        .unwrap_or_default();
    let name = arg_string(args, 0, context)?.to_ascii_lowercase();
    for line in headers.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(&name) {
            return Ok(js_string!(value.trim().to_string()).into());
        }
    }
    Ok(JsValue::null())
}

fn response_body(this: &JsValue) -> String {
    this.as_object()
        .and_then(|object| {
            object
                .downcast_ref::<ResponseKey>()
                .map(|key| key.body.clone())
        })
        .unwrap_or_default()
}

fn response_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = response_body(this);
    Ok(JsPromise::resolve(js_string!(body), context)?.into())
}

fn contains_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let ancestor = node_of(this).ok_or_else(type_error)?;
    let Some(node) = args.first().and_then(node_of) else {
        return Ok(JsValue::from(false));
    };
    Ok(JsValue::from(
        host.doc.borrow().contains_node(ancestor, node),
    ))
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

fn request_animation_frame(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let mut timed = args.to_vec();
    if timed.len() < 2 {
        timed.push(JsValue::from(0));
    }
    schedule_timer(context, &timed, false)
}

fn performance_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object
        .function(
            NativeFunction::from_fn_ptr(performance_now),
            js_string!("now"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_mark),
            js_string!("mark"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_mark),
            js_string!("measure"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_mark),
            js_string!("clearMarks"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_mark),
            js_string!("clearMeasures"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_entries),
            js_string!("getEntriesByName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(performance_entries),
            js_string!("getEntriesByType"),
            1,
        );
    Ok(object.build())
}

fn performance_mark(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn performance_entries(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsObject::from(JsArray::from_iter(Vec::<JsValue>::new(), context)).into())
}

fn performance_now(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(JsValue::from(host.clock.borrow().now as f64))
}

fn set_interval(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer(context, args, true)
}

fn clear_timer(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = args
        .first()
        .and_then(|value| value.as_number())
        .unwrap_or(0.0) as u32;
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

fn window_remove_listener(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    drop_listener(context, &format!("win:{event}"), &handler)?;
    Ok(JsValue::undefined())
}

fn document_listener(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let Some(html) = find_tag(&host.doc.borrow(), "html") else {
        return Ok(JsValue::undefined());
    };
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    push_listener(context, &format!("add:{}:{event}", html.0), &handler, false)?;
    Ok(JsValue::undefined())
}

fn document_remove_listener(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let Some(html) = find_tag(&host.doc.borrow(), "html") else {
        return Ok(JsValue::undefined());
    };
    let event = arg_string(args, 0, context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    drop_listener(context, &format!("add:{}:{event}", html.0), &handler)?;
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

fn get_document_element(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(find_tag(&host.doc.borrow(), "html")
        .map(|id| element_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn get_head(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(find_tag(&host.doc.borrow(), "head")
        .map(|id| element_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn create_text_node(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = arg_string(args, 0, context)?;
    let id = host.doc.borrow_mut().create_text_detached(&text);
    Ok(node_value(id.0, context))
}

fn node_value(id: usize, context: &mut Context) -> JsValue {
    let element = host(context)
        .ok()
        .is_some_and(|host| host.doc.borrow().element_name(NodeId(id)).is_some());
    if element {
        element_value(id, context)
    } else {
        text_node(id, context)
    }
}

fn text_node(id: usize, context: &mut Context) -> JsValue {
    if let Some(cached) = cached_node(id, context) {
        return cached;
    }
    let type_get = native(context, "get nodeType", 0, get_node_type);
    let name_get = native(context, "get nodeName", 0, get_node_name);
    let text_get = native(context, "get textContent", 0, get_text);
    let text_set = native(context, "set textContent", 1, set_text);
    let data_get = native(context, "get data", 0, get_text);
    let data_set = native(context, "set data", 1, set_text);
    let parent_get = native(context, "get parentNode", 0, get_parent_node);
    let next_get = native(context, "get nextSibling", 0, get_next_node);
    let previous_get = native(context, "get previousSibling", 0, get_previous_node);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("nodeType"), Some(type_get), None, ATTR)
        .accessor(js_string!("nodeName"), Some(name_get), None, ATTR)
        .accessor(js_string!("data"), Some(data_get), Some(data_set), ATTR)
        .accessor(
            js_string!("textContent"),
            Some(text_get),
            Some(text_set),
            ATTR,
        )
        .accessor(js_string!("parentNode"), Some(parent_get), None, ATTR)
        .accessor(js_string!("nextSibling"), Some(next_get), None, ATTR)
        .accessor(
            js_string!("previousSibling"),
            Some(previous_get),
            None,
            ATTR,
        );
    let value = JsValue::from(object.build());
    remember_node(id, &value, context);
    value
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

fn get_src(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let raw = host.doc.borrow().attr(id, "src").unwrap_or("").to_string();
    if raw.is_empty() {
        return Ok(js_string!("").into());
    }
    let base = host.location.borrow().clone();
    let absolute = resolve_request(&base, &raw)
        .map(|url| url.to_display())
        .unwrap_or(raw);
    Ok(js_string!(absolute).into())
}

fn set_src(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    write_attr(this, "src", args, context)
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

fn write_attr(
    this: &JsValue,
    name: &str,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, name, &value);
    Ok(JsValue::undefined())
}

fn get_checked(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(JsValue::from(
        host.doc.borrow().attr(id, "checked").is_some(),
    ))
}

fn set_checked(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let on = args
        .first()
        .and_then(|value| value.as_boolean())
        .unwrap_or(false);
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
    Ok(JsValue::from(selector_hits(
        &host.doc.borrow(),
        id,
        &selector,
    )))
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
    Ok(found
        .map(|id| element_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn selector_hits(doc: &Document, id: NodeId, selector: &str) -> bool {
    if doc.element_name(id).is_none() {
        return false;
    }
    let sheet = parse_css(&format!("{selector}{{}}"));
    sheet
        .rules
        .iter()
        .flat_map(|rule| &rule.selectors)
        .any(|selector| matches_element(doc, id, selector))
}

fn insert_before_child(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    let before = args.get(1).and_then(node_of);
    host.doc.borrow_mut().insert_before(parent, child, before);
    Ok(child_value)
}

fn remove_event_listener(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
    let child = if first {
        children.first().copied()
    } else {
        children.last().copied()
    };
    Ok(child
        .map(|id| node_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn get_next_node(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    sibling_node(this, context, true)
}

fn get_previous_node(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
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
            index
                .checked_sub(1)
                .and_then(|index| children.get(index).copied())
        }
    };
    Ok(sibling
        .map(|id| node_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn get_parent_node(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let parent = {
        let doc = host.doc.borrow();
        doc.node(id)
            .parent
            .filter(|parent| doc.element_name(*parent).is_some())
    };
    Ok(parent
        .map(|id| element_value(id.0, context))
        .unwrap_or(JsValue::null()))
}

fn get_current_target(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(host
        .event_current
        .get()
        .map(|id| node_value(id, context))
        .unwrap_or(JsValue::null()))
}

fn get_default_prevented(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(JsValue::from(host.prevented.get()))
}

fn listed_children(
    this: &JsValue,
    context: &mut Context,
    elements_only: bool,
) -> JsResult<JsValue> {
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
    bag.set(
        js_string!(key),
        JsObject::from(JsArray::from_iter(kept, context)),
        true,
        context,
    )?;
    Ok(())
}

fn import_map(doc: &Document) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for json in &doc.import_maps {
        collect_imports(json, &mut map);
    }
    map
}

fn collect_imports(json: &str, map: &mut HashMap<String, String>) {
    let Some(start) = json.find("\"imports\"") else {
        return;
    };
    let rest = &json[start + "\"imports\"".len()..];
    let Some(open) = rest.find('{') else {
        return;
    };
    let body = &rest[open + 1..];
    let bytes = body.as_bytes();
    let mut depth = 1usize;
    let mut index = 0usize;
    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                index += 1;
            }
            b'"' if depth == 1 => {
                let (key, next) = read_json_string(body, index);
                index = next;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                if index < bytes.len() && bytes[index] == b':' {
                    index += 1;
                }
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                if index < bytes.len() && bytes[index] == b'"' {
                    let (value, next) = read_json_string(body, index);
                    index = next;
                    if !key.is_empty() && !value.is_empty() {
                        map.insert(key, value);
                    }
                }
            }
            _ => index += 1,
        }
    }
}

fn read_json_string(text: &str, start: usize) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => {
                out.push(bytes[index + 1] as char);
                index += 2;
            }
            b'"' => return (out, index + 1),
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }
    (out, index)
}

fn map_specifier(imports: &HashMap<String, String>, spec: &str) -> Option<String> {
    if let Some(url) = imports.get(spec) {
        return Some(url.clone());
    }
    let mut best: Option<(&str, &str)> = None;
    for (key, value) in imports {
        if key.ends_with('/')
            && spec.starts_with(key.as_str())
            && best.is_none_or(|(current, _)| key.len() > current.len())
        {
            best = Some((key, value));
        }
    }
    best.map(|(key, value)| format!("{value}{}", &spec[key.len()..]))
}

impl ModuleLoader for PageLoader {
    fn init_import_meta(
        self: Rc<Self>,
        import_meta: &JsObject,
        module: &Module,
        context: &mut Context,
    ) {
        let url = module
            .path()
            .and_then(|path| path.to_str())
            .unwrap_or("")
            .to_string();
        let _ = import_meta.set(js_string!("url"), js_string!(url), false, context);
    }

    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl std::future::Future<Output = JsResult<Module>> {
        let requested = request.specifier().to_std_string_lossy();
        let mapped = map_specifier(&self.imports, &requested);
        let specifier = if let Some(mapped) = mapped {
            mapped
        } else {
            referrer
                .path()
                .and_then(|path| path.to_str())
                .and_then(|base| resolve_request(base, &requested).ok())
                .map(|url| url.to_display())
                .unwrap_or(requested)
        };
        let result = (|| {
            if let Some(module) = self.cache.borrow().get(&specifier).cloned() {
                return Ok(module);
            }
            let code = (self.fetch.borrow_mut())(&specifier).ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message(format!("module not found: {specifier}")),
                )
            })?;
            let path = PathBuf::from(&specifier);
            let module = {
                let mut context = context.borrow_mut();
                Module::parse(
                    Source::from_bytes(code.as_str()).with_path(&path),
                    None,
                    &mut context,
                )?
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
            if matches!(
                kind.as_str(),
                "submit" | "button" | "reset" | "image" | "file"
            ) {
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn host(context: &mut Context) -> JsResult<Host> {
    context.get_data::<Host>().cloned().ok_or_else(type_error)
}

fn type_error() -> JsError {
    JsNativeError::typ()
        .with_message("DOM object is missing")
        .into()
}

pub(crate) fn arg_string(
    args: &[JsValue],
    index: usize,
    context: &mut Context,
) -> JsResult<String> {
    let value = args.get(index).cloned().unwrap_or(JsValue::undefined());
    Ok(value.to_string(context)?.to_std_string_lossy())
}

pub(crate) fn node_of(value: &JsValue) -> Option<NodeId> {
    let object = value.as_object()?;
    let key = object.downcast_ref::<NodeKey>()?;
    Some(NodeId(key.id))
}

fn find_tag(doc: &Document, name: &str) -> Option<NodeId> {
    (0..doc.nodes.len())
        .map(NodeId)
        .find(|id| doc.element_name(*id) == Some(name))
}

fn query_first(
    doc: &Document,
    scope: NodeId,
    selector: &str,
    descendants_only: bool,
) -> Option<NodeId> {
    query_all(doc, scope, selector, descendants_only)
        .into_iter()
        .next()
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
    nodes
        .into_iter()
        .filter(|id| matches_element(doc, *id, parsed))
        .collect()
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
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

mod web;

#[cfg(test)]
mod tests {
    use super::*;
    use mithu_dom::NodeData;
    use mithu_html::parse_html;

    fn doc_name_is(node: &mithu_dom::Node, name: &str) -> bool {
        matches!(&node.data, NodeData::Element(element) if element.name == name)
    }

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
        assert_eq!(
            doc.get_element_by_id("msg")
                .map(|id| doc.text_content(id))
                .as_deref(),
            Some("after")
        );
        assert_eq!(doc.title, "Changed");
        let host_node = doc.get_element_by_id("host").unwrap();
        assert!(doc.has_class(host_node, "on"));
        let style = doc.attr(host_node, "style").unwrap_or("");
        assert!(style.contains("color: navy"), "{style}");
        assert!(style.contains("display: flex"), "{style}");
        assert_eq!(log, vec!["ready".to_string()]);
    }

    #[test]
    fn self_matches_window_and_frames_run() {
        let mut doc = parse_html(
            r#"<!doctype html><body><script>
            self.__next_f = self.__next_f || [];
            self.__next_f.push([0]);
            if (self !== window) throw new Error("self");
            var frame = document.createElement("iframe");
            frame.style.visibility = "hidden";
            frame.style.border = "none";
            document.body.appendChild(frame);
            requestAnimationFrame(function () {
              document.body.setAttribute("data-raf", String(performance.now()));
            });
            </script></body>"#,
        );
        let log = run_scripts(&mut doc, |_| None);
        assert!(log.is_empty(), "{log:?}");
        let body = doc
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (doc.element_name(NodeId(index)) == Some("body")).then_some(NodeId(index))
            })
            .unwrap();
        assert_eq!(doc.attr(body, "data-raf"), Some("0"));
        let frame = doc
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (doc.element_name(NodeId(index)) == Some("iframe")).then_some(NodeId(index))
            })
            .unwrap();
        let style = doc.attr(frame, "style").unwrap_or("");
        assert!(style.contains("visibility: hidden"), "{style}");
        assert!(style.contains("border: none"), "{style}");
    }

    #[test]
    fn comment_data_is_the_previous_sibling() {
        let mut doc = parse_html(
            "<div id='host'><!--$?--><span id='el'></span></div><script>
            var el = document.getElementById('el');
            var mark = el.previousSibling;
            if (!mark || mark.nodeType !== 8 || mark.data !== '$?') throw new Error(String(mark && mark.data));
            mark.data = '$~';
            </script>",
        );
        let log = run_scripts(&mut doc, |_| None);
        assert!(log.is_empty(), "{log:?}");
        let host = doc.get_element_by_id("host").unwrap();
        let comment = doc.node(host).children[0];
        assert_eq!(doc.text_content(comment), "$~");
    }

    #[test]
    fn example_script_inserts_translations_without_throwing() {
        let script = r#"var B=document.body,P,p;B.children[0].insertAdjacentHTML("afterend","<p lang=ar>AR</p><p lang=fr>FR</p><a href=https://iana.org/help/example-domains>Learn more</a>");P=[...B.querySelectorAll("p")];P[0].lang="en";navigator.languages.some(l=>p=P.find(p=>p.lang==l.split("-")[0]));p=p||P[0];B.prepend(p);B.insertAdjacentHTML("afterbegin","<style>p+p{opacity:.6}</style><svg viewBox=0,0,20,20></svg>");"#;
        let mut doc = parse_html(&format!(
            "<!doctype html><html><head><title>Example Domain</title></head><body><p>This domain is for use in documentation examples.</p><script>{script}</script></body></html>"
        ));
        let log = run_scripts(&mut doc, |_| None);
        assert!(log.is_empty(), "{log:?}");
        let body = doc
            .nodes
            .iter()
            .position(|node| doc_name_is(node, "body"))
            .map(mithu_dom::NodeId)
            .expect("body");
        let text = doc.text_content(body);
        assert!(text.contains("AR"), "{text}");
        assert!(text.contains("FR"), "{text}");
        assert!(text.contains("Learn more"), "{text}");
        assert!(
            doc.nodes.iter().any(|node| doc_name_is(node, "svg")),
            "svg from insertAdjacentHTML"
        );
        assert!(
            doc.stylesheets
                .iter()
                .any(|sheet| sheet.contains("opacity"))
                || doc.nodes.iter().any(|node| doc_name_is(node, "style")),
            "style element should be inserted"
        );
        doc.refresh_metadata();
        assert!(
            doc.stylesheets
                .iter()
                .any(|sheet| sheet.contains("opacity")),
            "{:?}",
            doc.stylesheets
        );
    }

    #[test]
    fn inner_html_builds_elements() {
        let mut doc = parse_html(
            r##"<div id="host"></div><script>
            document.getElementById("host").innerHTML = "<b id='yes'>Hi</b>";
        </script>"##,
        );
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
    fn an_external_script_sees_its_url_and_then_the_document_loads() {
        let doc = parse_html(
            r#"<script src="/app.js"></script>
            <script>
              window.addEventListener("load", function () {
                document.title = document.title + "|loaded";
              });
            </script>"#,
        );
        let session = Session::open_at(doc, "https://example.com/dir/page", |src| {
            (src == "/app.js").then(|| {
                "document.title = document.currentScript.src + '|' + document.currentScript.getAttribute('src');".to_string()
            })
        });
        assert!(
            session.logs().is_empty(),
            "load should not throw: {:?}",
            session.logs()
        );
        assert_eq!(
            session.document().title,
            "https://example.com/app.js|/app.js|loaded"
        );
    }

    #[test]
    fn github_banner_script_reads_cookie() {
        let doc = parse_html("<script src=\"/banner.js\"></script>");
        let session = Session::open_at(doc, "https://github.com/", |src| {
            (src == "/banner.js").then(|| {
                r#"performance.mark("js-parse-end");let e,t,n,l,a="global-banner-disable-styles",d=/^[a-zA-Z0-9_-]+$/;n=((t=(e=document.cookie.match(RegExp("(?:^|;\\s*)disabled_global_site_banners\\s*=\\s*([^;]+)")))?e[1]:null)?t.split(",").map(e=>e.trim()).filter(e=>d.test(e)):[]).map(e=>`global-banner[id="${e}"]{display:none !important}`).join(""),l=document.getElementById(a),n?(l||((l=document.createElement("style")).id=a,document.head.appendChild(l)),l.textContent=n):l?.remove();"#.to_string()
            })
        });
        assert!(session.logs().is_empty(), "{:?}", session.logs());
    }

    #[test]
    fn a_large_script_runs_and_a_long_loop_yields() {
        let filler = " ".repeat(300_000);
        let doc = parse_html(&format!(
            "<script>document.title = 'big';{filler}</script><script>var i = 0; while (i < 200000) i = i + 1; document.title = document.title + i;</script>"
        ));
        let mut session = Session::open_cooperative(
            doc,
            "https://example.com/",
            |_| None,
            HashMap::new(),
            HashMap::new(),
        );
        assert!(session.scripts_pending());
        let mut slices = 0;
        while session.pump(Duration::ZERO) {
            slices += 1;
            assert!(slices < 100_000, "the script task never yielded");
        }
        assert!(slices > 0, "a long loop should cross a budget boundary");
        assert_eq!(session.document().title, "big200000");
        assert!(session.logs().is_empty(), "{:?}", session.logs());
    }

    #[test]
    fn document_cookie_is_a_string() {
        let doc = parse_html(
            r#"<script>document.title = String(document.cookie.match(/a=b/));</script>"#,
        );
        let session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "null");
    }

    #[test]
    fn a_module_imports_a_relative_url() {
        let doc = parse_html(r#"<script type="module" src="/app.js"></script>"#);
        let session = Session::open_at(doc, "https://example.com/docs/page", |src| match src {
            "/app.js" => {
                Some("import { name } from './lib.js'; document.title = name;".to_string())
            }
            "https://example.com/lib.js" => Some("export const name = 'mithu';".to_string()),
            other => Some(format!("missing {other}")),
        });
        assert!(
            session.logs().is_empty(),
            "module import failed: {:?}",
            session.logs()
        );
        assert_eq!(session.document().title, "mithu");
    }

    #[test]
    fn current_script_is_an_element_with_get_attribute() {
        let doc = parse_html(r#"<script src="/chunk.js"></script>"#);
        let session = Session::open_at(doc, "https://foisal.org/page", |src| {
            (src == "/chunk.js").then(|| {
                r#"globalThis.TURBOPACK={push:function(chunk){
                    var script=chunk[0];
                    document.title=script.getAttribute("src")+"|"+script.src;
                }};
                (globalThis.TURBOPACK||(globalThis.TURBOPACK=[])).push([document.currentScript,1]);"#
                    .to_string()
            })
        });
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(
            session.document().title,
            "/chunk.js|https://foisal.org/chunk.js"
        );
    }

    #[test]
    fn url_text_encoder_history_and_observer_exist() {
        let doc = parse_html(
            r#"<script>
              var url = new URL("/docs?q=mithu+browser#top", "https://example.com/a/b");
              var path = url.pathname;
              var query = url.searchParams.get("q");
              var hash = url.hash;
              var encoded = new TextEncoder().encode("Hi");
              var decoded = new TextDecoder().decode(encoded);
              var latin = atob(btoa("mithu"));
              history.pushState(null, "", "/next");
              history.replaceState(null, "", "/replaced");
              new MutationObserver(function () {});
              document.title = path + "|" + query + "|" + hash + "|" + decoded + "|" + latin + "|" + location.href;
            </script>"#,
        );
        let session = Session::open_at(doc, "https://example.com/a/b", |_| None);
        assert!(
            session.logs().is_empty(),
            "host classes should not throw: {:?}",
            session.logs()
        );
        assert_eq!(
            session.document().title,
            "/docs|mithu browser|#top|Hi|mithu|https://example.com/replaced"
        );
    }

    #[test]
    fn local_storage_round_trips_and_json_scripts_are_skipped() {
        let mut doc = parse_html(
            r#"<script type="application/ld+json">{not js}</script>
            <script>
              localStorage.setItem("city", "Dhaka");
              sessionStorage.setItem("tab", "1");
            </script>"#,
        );
        let session = Session::open(std::mem::take(&mut doc), |_| None);
        assert_eq!(
            session.local_storage().get("city").map(String::as_str),
            Some("Dhaka")
        );
        assert_eq!(
            session.session_storage().get("tab").map(String::as_str),
            Some("1")
        );
        assert!(
            session.logs().iter().all(|line| !line.contains("not js")),
            "a data block must not be evaluated"
        );
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
        assert!(
            doc.parent_element(doc.get_element_by_id("b").unwrap())
                .is_none()
        );
        assert_eq!(
            doc.parent_element(doc.get_element_by_id("a").unwrap()),
            Some(host)
        );
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
        assert_eq!(
            session.take_navigation().as_deref(),
            Some("https://example.com/next")
        );
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

    #[test]
    fn pages_get_image_ports_crypto_and_import_maps() {
        let doc = parse_html(
            r#"<body id="page"></body>
            <script type="importmap">{"imports":{"react-dom/client":"https://cdn.example/client.js"}}</script>
            <script type="module">
              import { name } from "react-dom/client";
              document.documentElement.setAttribute("data-mod", name);
              document.documentElement.setAttribute("data-meta", import.meta.url);
            </script>
            <script>
              var img = new Image();
              img.src = "https://example.com/pixel.png";
              var channel = new MessageChannel();
              channel.port1.onmessage = function (event) { img.alt = event.data; };
              channel.port2.postMessage("ported");
              var bytes = new Uint8Array(4);
              crypto.getRandomValues(bytes);
              var link = document.createElement("a");
              link.href = "https://cdn.example/app?q=1#top";
              document.documentElement.dataset.theme = "dark";
              var order = "s";
              var xhr = new XMLHttpRequest();
              xhr.open("GET", "about:blank");
              xhr.onload = function () { order += "l"; };
              xhr.send();
              order += "d";
              requestIdleCallback(function (deadline) { document.getElementById("page").setAttribute("data-idle", String(deadline.timeRemaining())); });
              setTimeout(function () {
                var box = document.getElementById("page").getBoundingClientRect();
                document.getElementById("page").setAttribute("data-box", String(box.width) + "x" + document.getElementById("page").clientWidth);
              }, 1);
              var observer = new MutationObserver(function (records) {
                document.getElementById("page").setAttribute("data-seen", records[0].type);
              });
              observer.observe(document.getElementById("page"), { attributes: true });
              document.getElementById("page").setAttribute("data-x", "1");
              var style = document.createElement("style");
              document.body.appendChild(style);
              style.sheet.insertRule("a{}", 0);
              document.title = [
                img.src,
                img.alt,
                crypto.randomUUID().length,
                link.host,
                location.search,
                document.referrer,
                document.documentElement.getAttribute("data-theme"),
                order,
                typeof fetch,
                typeof HTMLElement,
                typeof Document,
                document instanceof Document,
                navigator.platform,
                typeof window.removeEventListener,
                new Event("ping").type,
                document.body === document.body,
                document.styleSheets.length
              ].join("|");
            </script>"#,
        );
        let mut session = Session::open_at(doc, "https://example.com/docs?q=mithu", |src| {
            (src == "https://cdn.example/client.js")
                .then(|| "export const name = \"react\";".to_string())
        });
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        let title = session.document().title.clone();
        assert_eq!(
            title,
            "https://example.com/pixel.png|ported|36|cdn.example|?q=mithu||dark|sd|function|function|function|true|Linux x86_64|function|ping|true|1"
        );
        let html = session
            .document()
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (session.document().element_name(mithu_dom::NodeId(index)) == Some("html"))
                    .then_some(mithu_dom::NodeId(index))
            })
            .unwrap();
        assert_eq!(session.document().attr(html, "data-mod"), Some("react"));
        assert_eq!(
            session.document().attr(html, "data-meta"),
            Some("inline-module.js")
        );
        let page = session.document().get_element_by_id("page").unwrap();
        assert_eq!(
            session.document().attr(page, "data-seen"),
            Some("attributes")
        );
        let mut boxes = std::collections::HashMap::new();
        boxes.insert(
            page.0,
            BoxMetrics {
                x: 8.0,
                y: 16.0,
                width: 320.0,
                height: 40.0,
                border_left: 2.0,
                border_right: 2.0,
                ..BoxMetrics::default()
            },
        );
        session.set_geometry(boxes, 0.0);
        assert!(session.advance(5));
        assert_eq!(session.document().attr(page, "data-idle"), Some("50"));
        assert_eq!(session.document().attr(page, "data-box"), Some("320x316"));
    }
}
