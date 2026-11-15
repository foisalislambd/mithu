//! In-process JavaScript virtual machine.
//!
//! Boa runs ECMAScript in this process, including modules. Host objects are the
//! window, document, and node tree. Timers and `location` report back to the browser.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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
    /// Script source by the path Boa prints in a stack, for `MITHU_JS_DEBUG`.
    sources: Rc<RefCell<HashMap<String, String>>>,
    loader: Rc<PageLoader>,
    /// Parser-inserted scripts, so a later move does not run them again.
    ran_scripts: Rc<RefCell<HashSet<usize>>>,
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
    /// Wall time at the last timer jump, so `performance.now()` keeps moving
    /// during a task and still jumps when the event loop advances.
    wall: std::time::Instant,
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
            wall: std::time::Instant::now(),
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
            .host_hooks(Rc::new(RejectionHooks))
            .build()
            .unwrap_or_else(|_| Context::default());
        // Boa's default stack is 10,240 slots. A page script that builds a
        // large expression (Google's interstitial) hits that before it finishes.
        let mut limits = boa_engine::vm::RuntimeLimits::default();
        limits.set_stack_size_limit(1024 * 1024);
        // Each Boa frame is several kilobytes of native stack. 4,096 frames
        // overflow the UI thread before Boa can throw. 1,536 still covers a
        // deep render and turns a runaway constructor into a catchable error.
        limits.set_recursion_limit(1_536);
        context.set_runtime_limits(limits);
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
            ready_state: Rc::new(RefCell::new("loading".to_string())),
            geometry: Rc::new(RefCell::new(HashMap::new())),
            scroll: Rc::new(Cell::new(0.0)),
            sources: Rc::new(RefCell::new(HashMap::new())),
            loader: Rc::clone(&loader),
            ran_scripts: Rc::new(RefCell::new(HashSet::new())),
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

struct RejectionHooks;

impl boa_engine::context::HostHooks for RejectionHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject<boa_engine::builtins::promise::Promise>,
        operation: boa_engine::builtins::promise::OperationType,
        context: &mut Context,
    ) {
        let promise = JsValue::from(promise.clone());
        let Some(object) = promise.as_object() else {
            return;
        };
        let unhandled = operation == boa_engine::builtins::promise::OperationType::Reject;
        let _ = object.set(
            js_string!("__mithuUnhandled"),
            JsValue::from(unhandled),
            false,
            context,
        );
        if !unhandled {
            return;
        }
        // A handler may attach before the current turn ends. Report only if
        // the rejection is still unhandled when that turn's microtasks run.
        let Ok(report) = context
            .global_object()
            .get(js_string!("__mithuReportRejection"), context)
        else {
            return;
        };
        if let Some(report) = report.as_object() {
            let _ = report.call(&JsValue::undefined(), &[promise], context);
        }
    }
}

fn record_rejection(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let reason = args.first().cloned().unwrap_or(JsValue::undefined());
    let mut message = reason.display().to_string();
    if let Some(object) = reason.as_object()
        && let Ok(stack) = object.get(js_string!("stack"), context)
    {
        message.push('\n');
        message.push_str(&stack.display().to_string());
    }
    note_error(context, &message);
    Ok(JsValue::undefined())
}

fn note_error(context: &Context, error: &dyn std::fmt::Display) {
    if let Some(host) = context.get_data::<Host>() {
        let message = format!("JavaScript error: {error}");
        let extra = debug_source_window(host, &message);
        host.log.borrow_mut().push(format!("{message}{extra}"));
    }
}

fn remember_source(host: &Host, path: &str, code: &str) {
    if std::env::var_os("MITHU_JS_DEBUG").is_none() {
        return;
    }
    let mut sources = host.sources.borrow_mut();
    sources.insert(path.to_string(), code.to_string());
    if let Some(name) = path.rsplit(['/', '\\']).next() {
        sources.insert(name.to_string(), code.to_string());
    }
}

/// `MITHU_JS_DEBUG=1` appends the source text around each stack location.
fn debug_source_window(host: &Host, message: &str) -> String {
    if std::env::var_os("MITHU_JS_DEBUG").is_none() {
        return String::new();
    }
    let sources = host.sources.borrow();
    for token in message.split_whitespace() {
        let token = token.trim_matches(|ch: char| ch == '(' || ch == ')');
        // Stack frames look like `https://host/a.js:12:34`. The line and
        // column are the last two colon fields so the URL's scheme survives.
        let Some((path_and_line, column)) = token.rsplit_once(':') else {
            continue;
        };
        let Ok(col) = column
            .trim_end_matches(|ch: char| !ch.is_ascii_digit())
            .parse::<usize>()
        else {
            continue;
        };
        let Some((path, line_text)) = path_and_line.rsplit_once(':') else {
            continue;
        };
        let Ok(line) = line_text.parse::<usize>() else {
            continue;
        };
        if line == 0 {
            continue;
        }
        let source = sources.get(path).or_else(|| {
            sources
                .iter()
                .find(|(key, _)| key.ends_with(path) || path.ends_with(key.as_str()))
                .map(|(_, code)| code)
        });
        let Some(source) = source else {
            continue;
        };
        let Some(text) = source.lines().nth(line - 1) else {
            continue;
        };
        let at = col.saturating_sub(1).min(text.len());
        let start = at.saturating_sub(140);
        let end = (at + 80).min(text.len());
        let slice = text.get(start..end).unwrap_or("");
        return format!("\njs-debug {path}:{line}:{col}: {slice}");
    }
    String::new()
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
                host.ran_scripts.borrow_mut().insert(job.node.0);
                remember_source(host, &job.path, &job.code);
            }
            if std::env::var_os("MITHU_JS_DEBUG").is_some() {
                eprintln!(
                    "script {} {}",
                    if job.module { "module" } else { "classic" },
                    job.path
                );
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
    context.register_global_property(js_string!("self"), window.clone(), flags)?;
    // A top-level page is its own browsing context. YouTube reads
    // `window.top[name]` and throws when `top` is missing.
    context.register_global_property(js_string!("top"), window.clone(), flags)?;
    context.register_global_property(js_string!("parent"), window.clone(), flags)?;
    context.register_global_property(js_string!("frames"), window.clone(), flags)?;
    context.register_global_property(js_string!("frameElement"), JsValue::null(), flags)?;
    context.register_global_property(js_string!("length"), JsValue::from(0), flags)?;
    let performance = performance_object(context)?;
    context.register_global_property(js_string!("performance"), performance, flags)?;
    context.register_global_property(js_string!("navigator"), navigator, flags)?;
    let screen = screen_object(context)?;
    context.register_global_property(js_string!("screen"), screen, flags)?;
    context.register_global_property(js_string!("innerWidth"), JsValue::from(1280), flags)?;
    context.register_global_property(js_string!("innerHeight"), JsValue::from(800), flags)?;
    context.register_global_property(js_string!("outerWidth"), JsValue::from(1280), flags)?;
    context.register_global_property(js_string!("outerHeight"), JsValue::from(800), flags)?;
    context.register_global_property(js_string!("devicePixelRatio"), JsValue::from(1), flags)?;
    context.register_global_property(js_string!("location"), location.clone(), flags)?;
    if let Ok(document) = context.global_object().get(js_string!("document"), context)
        && let Some(document) = document.as_object()
    {
        // `document.location` is the same object as `window.location`.
        let _ = document.set(js_string!("location"), location, false, context);
    }
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
    context.register_global_callable(
        js_string!("dispatchEvent"),
        1,
        NativeFunction::from_fn_ptr(window_dispatch),
    )?;
    context.register_global_callable(
        js_string!("postMessage"),
        1,
        NativeFunction::from_fn_ptr(window_post_message),
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
    if let Ok(script) = Script::parse(Source::from_bytes(LANGUAGE_EXTRAS), None, context) {
        let _ = script.evaluate(context);
    }
    let _ = wire_document_prototype(context);
    let _ = wire_prototype_chain(context);
    let _ = install_group(context);
    context.register_global_callable(
        js_string!("__mithuRecordRejection"),
        1,
        NativeFunction::from_fn_ptr(record_rejection),
    )?;
    context.eval(Source::from_bytes(READABLE_STREAM))?;
    context.eval(Source::from_bytes(include_str!("platform.js")))?;
    Ok(())
}

/// Enough of `ReadableStream` for a React flight stream: enqueue, close,
/// `getReader().read()`, and `tee()`.
const READABLE_STREAM: &str = r#"
class ReadableStream {
  constructor(underlying) {
    this._queue = [];
    this._closed = false;
    this._error = undefined;
    this._wait = null;
    var stream = this;
    this._controller = {
      enqueue: function (chunk) {
        stream._queue.push(chunk);
        stream._wake();
      },
      close: function () {
        stream._closed = true;
        stream._wake();
      },
      error: function (err) {
        stream._error = err;
        stream._closed = true;
        stream._wake();
      },
      get desiredSize() {
        return stream._closed ? 0 : 1;
      }
    };
    if (underlying && typeof underlying.start === "function") {
      underlying.start(this._controller);
    }
  }
  _wake() {
    var wait = this._wait;
    if (!wait) return;
    if (this._error) {
      this._wait = null;
      wait.reject(this._error);
      return;
    }
    if (this._queue.length) {
      this._wait = null;
      wait.resolve({ value: this._queue.shift(), done: false });
      return;
    }
    if (this._closed) {
      this._wait = null;
      wait.resolve({ value: undefined, done: true });
    }
  }
  getReader() {
    var stream = this;
    return {
      read: function () {
        if (stream._error) return Promise.reject(stream._error);
        if (stream._queue.length) {
          return Promise.resolve({ value: stream._queue.shift(), done: false });
        }
        if (stream._closed) return Promise.resolve({ value: undefined, done: true });
        return new Promise(function (resolve, reject) {
          stream._wait = { resolve: resolve, reject: reject };
        });
      },
      releaseLock: function () {},
      cancel: function () {
        stream._controller.close();
        return Promise.resolve();
      }
    };
  }
  cancel() {
    this._controller.close();
    return Promise.resolve();
  }
  tee() {
    var reader = this.getReader();
    var first, second;
    var left = new ReadableStream({ start: function (controller) { first = controller; } });
    var right = new ReadableStream({ start: function (controller) { second = controller; } });
    var pump = function () {
      reader.read().then(function (result) {
        if (result.done) {
          first.close();
          second.close();
          return;
        }
        first.enqueue(result.value);
        second.enqueue(result.value);
        pump();
      }, function (err) {
        first.error(err);
        second.error(err);
      });
    };
    pump();
    return [left, right];
  }
}
globalThis.__mithuReportRejection = function (promise) {
  Promise.resolve().then(function () {
    if (promise.__mithuUnhandled) {
      promise.catch(function (reason) { globalThis.__mithuRecordRejection(reason); });
    }
  });
};
"#;

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

fn wire_prototype_chain(context: &mut Context) -> JsResult<()> {
    // Blink's chain: EventTarget <- Node <- Element <- HTMLElement <- HTMLTemplateElement.
    // Document is also a Node. `instanceof` walks this chain.
    for (child, parent) in [
        ("HTMLTemplateElement", "HTMLElement"),
        ("HTMLImageElement", "HTMLElement"),
        ("HTMLElement", "Element"),
        ("Element", "Node"),
        ("Document", "Node"),
        ("DocumentFragment", "Node"),
        ("Node", "EventTarget"),
        ("ShadowRoot", "DocumentFragment"),
    ] {
        link_prototype(context, child, parent)?;
    }
    for name in crate::web::HTML_ELEMENT_CLASSES {
        link_prototype(context, name, "HTMLElement")?;
    }
    link_prototype(context, "HTMLMediaElement", "HTMLElement")?;
    link_prototype(context, "HTMLAudioElement", "HTMLMediaElement")?;
    link_prototype(context, "HTMLVideoElement", "HTMLMediaElement")?;
    install_node_methods(context)?;
    install_canvas_methods(context)?;
    expose_shady_natives(context)?;
    Ok(())
}

/// Web components copy `addEventListener` off `EventTarget.prototype`, then call
/// `window.__shady_native_addEventListener`. The global object is not a `Window`
/// instance, so the copied prototype property would be invisible.
fn expose_shady_natives(context: &mut Context) -> JsResult<()> {
    link_prototype(context, "Window", "EventTarget")?;
    let global = context.global_object();
    for name in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        let value = global.get(js_string!(name), context)?;
        if value.is_undefined() || value.is_null() {
            continue;
        }
        global.set(
            js_string!(format!("__shady_native_{name}")),
            value.clone(),
            true,
            context,
        )?;
        let window = global.get(js_string!("Window"), context)?;
        if let Some(window) = window.as_object() {
            let proto = window.get(js_string!("prototype"), context)?;
            if let Some(proto) = proto.as_object() {
                proto.set(js_string!(name), value, true, context)?;
            }
        }
    }
    let window = context.global_object().get(js_string!("Window"), context)?;
    if let Some(window) = window.as_object() {
        let proto = window.get(js_string!("prototype"), context)?;
        if let Some(proto) = proto.as_object() {
            context.global_object().set_prototype(Some(proto.clone()));
        }
    }
    Ok(())
}

fn install_node_methods(context: &mut Context) -> JsResult<()> {
    let node = context.global_object().get(js_string!("Node"), context)?;
    let Some(node) = node.as_object() else {
        return Ok(());
    };
    let proto = node.get(js_string!("prototype"), context)?;
    let Some(proto) = proto.as_object() else {
        return Ok(());
    };
    let append = native(context, "appendChild", 1, append_child);
    proto.set(js_string!("appendChild"), append, true, context)?;
    let insert = native(context, "insertBefore", 2, insert_before_child);
    proto.set(js_string!("insertBefore"), insert, true, context)?;
    let remove = native(context, "removeChild", 1, remove_child);
    proto.set(js_string!("removeChild"), remove, true, context)?;
    // These four are not also own properties of each element. An enumerable
    // prototype method shows up in `for-in` and `getOwnPropertyDescriptor`
    // returns undefined, which is the Zod `.get` throw on a host object.
    // Blink's for-in on a fresh element does not surface them that way.
    let equal = native(context, "isEqualNode", 1, is_equal_node);
    define_hidden(&proto, context, "isEqualNode", JsValue::from(equal))?;
    let same = native(context, "isSameNode", 1, is_same_node);
    define_hidden(&proto, context, "isSameNode", JsValue::from(same))?;
    let clone = native(context, "cloneNode", 1, clone_node);
    define_hidden(&proto, context, "cloneNode", JsValue::from(clone))?;
    let replace = native(context, "replaceChild", 2, replace_child);
    define_hidden(&proto, context, "replaceChild", JsValue::from(replace))?;
    let text_get = native(context, "get textContent", 0, get_text);
    let text_set = native(context, "set textContent", 1, set_text);
    define_accessor(&proto, context, "textContent", text_get, Some(text_set))?;
    let parent_node = native(context, "get parentNode", 0, get_parent_node);
    let parent_element = native(context, "get parentElement", 0, get_parent_node);
    let first_child = native(context, "get firstChild", 0, get_first_child);
    let last_child = native(context, "get lastChild", 0, get_last_child);
    let next_sibling = native(context, "get nextSibling", 0, get_next_sibling);
    let previous_sibling = native(context, "get previousSibling", 0, get_previous_sibling);
    let child_nodes = native(context, "get childNodes", 0, get_child_nodes);
    define_accessor(&proto, context, "parentNode", parent_node, None)?;
    define_accessor(&proto, context, "parentElement", parent_element, None)?;
    define_accessor(&proto, context, "firstChild", first_child, None)?;
    define_accessor(&proto, context, "lastChild", last_child, None)?;
    define_accessor(&proto, context, "nextSibling", next_sibling, None)?;
    define_accessor(&proto, context, "previousSibling", previous_sibling, None)?;
    define_accessor(&proto, context, "childNodes", child_nodes, None)?;
    Ok(())
}

fn install_group(context: &mut Context) -> JsResult<()> {
    let array = context.global_object().get(js_string!("Array"), context)?;
    if let Some(array) = array.as_object() {
        let proto = array.get(js_string!("prototype"), context)?;
        if let Some(proto) = proto.as_object() {
            let group = native(context, "group", 1, group_array);
            let property = boa_engine::property::PropertyDescriptor::builder()
                .value(group)
                .writable(true)
                .enumerable(false)
                .configurable(true);
            proto.define_property_or_throw(js_string!("group"), property, context)?;
        }
    }
    let object = context.global_object().get(js_string!("Object"), context)?;
    if let Some(object) = object.as_object() {
        let group_by = native(context, "groupBy", 2, group_by_object);
        object.set(js_string!("groupBy"), group_by, true, context)?;
    }
    let map = context.global_object().get(js_string!("Map"), context)?;
    if let Some(map) = map.as_object() {
        let group_by = native(context, "groupBy", 2, group_by_map);
        map.set(js_string!("groupBy"), group_by, true, context)?;
    }
    Ok(())
}

fn group_array(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or(JsValue::undefined());
    let this_arg = args.get(1).cloned().unwrap_or(JsValue::undefined());
    groups_to_object(this, &callback, &this_arg, context)
}

fn group_by_object(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let items = args.first().cloned().unwrap_or(JsValue::undefined());
    let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let this_arg = args.get(2).cloned().unwrap_or(JsValue::undefined());
    groups_to_object(&items, &callback, &this_arg, context)
}

fn group_by_map(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let items = args.first().cloned().unwrap_or(JsValue::undefined());
    let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let this_arg = args.get(2).cloned().unwrap_or(JsValue::undefined());
    let groups = collect_groups(&items, &callback, &this_arg, context)?;
    let ctor = context.global_object().get(js_string!("Map"), context)?;
    let Some(ctor) = ctor.as_object() else {
        return Ok(JsValue::undefined());
    };
    let map = ctor.construct(&[], None, context)?;
    let set = map.get(js_string!("set"), context)?;
    let Some(set) = set.as_object() else {
        return Ok(JsValue::from(map));
    };
    for (key, items) in groups {
        let array = JsValue::from(JsArray::from_iter(items, context));
        set.call(&JsValue::from(map.clone()), &[key, array], context)?;
    }
    Ok(JsValue::from(map))
}

fn groups_to_object(
    items: &JsValue,
    callback: &JsValue,
    this_arg: &JsValue,
    context: &mut Context,
) -> JsResult<JsValue> {
    let groups = collect_groups(items, callback, this_arg, context)?;
    let object = ObjectInitializer::new(context).build();
    for (key, values) in groups {
        let name = key.to_string(context)?;
        let array = JsValue::from(JsArray::from_iter(values, context));
        object.set(name, array, true, context)?;
    }
    Ok(object.into())
}

fn collect_groups(
    items: &JsValue,
    callback: &JsValue,
    this_arg: &JsValue,
    context: &mut Context,
) -> JsResult<Vec<(JsValue, Vec<JsValue>)>> {
    let Some(callback) = callback
        .as_object()
        .filter(|function| function.is_callable())
    else {
        return Err(JsNativeError::typ()
            .with_message("group callback must be a function")
            .into());
    };
    let Some(object) = items.as_object() else {
        return Ok(Vec::new());
    };
    let length = object
        .get(js_string!("length"), context)?
        .as_number()
        .unwrap_or(0.0);
    let length = if length.is_finite() && length > 0.0 {
        length as usize
    } else {
        0
    };
    let mut groups: Vec<(String, JsValue, Vec<JsValue>)> = Vec::new();
    for index in 0..length {
        let item = object.get(index as u32, context)?;
        let key = callback.call(
            this_arg,
            &[item.clone(), JsValue::from(index as u32), items.clone()],
            context,
        )?;
        let name = key.to_string(context)?.to_std_string_lossy();
        if let Some(group) = groups.iter_mut().find(|(existing, _, _)| existing == &name) {
            group.2.push(item);
        } else {
            groups.push((name, key, vec![item]));
        }
    }
    Ok(groups
        .into_iter()
        .map(|(_, key, values)| (key, values))
        .collect())
}

fn install_canvas_methods(context: &mut Context) -> JsResult<()> {
    let ctor = context
        .global_object()
        .get(js_string!("HTMLCanvasElement"), context)?;
    let Some(ctor) = ctor.as_object() else {
        return Ok(());
    };
    let proto = ctor.get(js_string!("prototype"), context)?;
    let Some(proto) = proto.as_object() else {
        return Ok(());
    };
    let get_context = native(context, "getContext", 1, canvas_get_context);
    proto.set(js_string!("getContext"), get_context, true, context)?;
    let to_data = native(context, "toDataURL", 0, canvas_to_data_url);
    proto.set(js_string!("toDataURL"), to_data, true, context)?;
    Ok(())
}

fn canvas_get_context(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let kind = arg_string(args, 0, context).unwrap_or_else(|_| "2d".to_string());
    if kind == "2d" || kind.is_empty() {
        return Ok(canvas_2d(context)?.into());
    }
    Ok(JsValue::null())
}

fn canvas_to_data_url(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(js_string!("data:,").into())
}

fn canvas_noop(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn canvas_measure(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, context).unwrap_or_default();
    let width = (text.chars().count() as f64).max(1.0);
    Ok(ObjectInitializer::new(context)
        .property(js_string!("width"), JsValue::from(width), ATTR)
        .build()
        .into())
}

/// A 2d context that accepts the drawing calls pages make while probing
/// canvas support. Nothing is rasterized.
fn canvas_2d(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    for name in [
        "fillRect",
        "clearRect",
        "strokeRect",
        "fillText",
        "strokeText",
        "beginPath",
        "closePath",
        "moveTo",
        "lineTo",
        "arc",
        "rect",
        "fill",
        "stroke",
        "save",
        "restore",
        "translate",
        "scale",
        "rotate",
        "transform",
        "setTransform",
        "drawImage",
        "putImageData",
        "createLinearGradient",
        "createRadialGradient",
        "createPattern",
    ] {
        object.function(
            NativeFunction::from_fn_ptr(canvas_noop),
            js_string!(name.to_string()),
            0,
        );
    }
    object
        .function(
            NativeFunction::from_fn_ptr(canvas_measure),
            js_string!("measureText"),
            1,
        )
        .property(js_string!("fillStyle"), js_string!("#000000"), ATTR)
        .property(js_string!("strokeStyle"), js_string!("#000000"), ATTR)
        .property(js_string!("font"), js_string!("10px sans-serif"), ATTR)
        .property(js_string!("textAlign"), js_string!("start"), ATTR)
        .property(js_string!("lineWidth"), JsValue::from(1), ATTR)
        .property(js_string!("globalAlpha"), JsValue::from(1), ATTR);
    Ok(object.build())
}

fn define_hidden(
    proto: &JsObject,
    context: &mut Context,
    name: &str,
    value: JsValue,
) -> JsResult<()> {
    let property = boa_engine::property::PropertyDescriptor::builder()
        .value(value)
        .writable(true)
        .enumerable(false)
        .configurable(true);
    proto
        .define_property_or_throw(js_string!(name.to_string()), property, context)
        .map(|_| ())
}

pub(crate) fn define_accessor(
    proto: &JsObject,
    context: &mut Context,
    name: &str,
    getter: boa_engine::object::builtins::JsFunction,
    setter: Option<boa_engine::object::builtins::JsFunction>,
) -> JsResult<()> {
    let property = boa_engine::property::PropertyDescriptor::builder()
        .get(JsValue::from(getter))
        .maybe_set(setter.map(JsValue::from))
        .enumerable(true)
        .configurable(true);
    proto
        .define_property_or_throw(js_string!(name.to_string()), property, context)
        .map(|_| ())
}

fn link_prototype(context: &mut Context, child: &str, parent: &str) -> JsResult<()> {
    let child = context
        .global_object()
        .get(js_string!(child.to_string()), context)?;
    let parent = context
        .global_object()
        .get(js_string!(parent.to_string()), context)?;
    let (Some(child), Some(parent)) = (child.as_object(), parent.as_object()) else {
        return Ok(());
    };
    let child_proto = child.get(js_string!("prototype"), context)?;
    let parent_proto = parent.get(js_string!("prototype"), context)?;
    if let (Some(child_proto), Some(parent_proto)) =
        (child_proto.as_object(), parent_proto.as_object())
    {
        child_proto.set_prototype(Some(parent_proto.clone()));
    }
    Ok(())
}

const LANGUAGE_EXTRAS: &str = r#"
(function () {
  var nativeToString = Function.prototype.toString;
  Function.prototype.toString = function () {
    var text = nativeToString.call(this);
    // Boa prints a class with no saved source as a native function.
    // Only anonymous classes get the class form, so host methods such
    // as attachShadow still contain "[native code]".
    if (text === "function () { [native code] }" && this.name === "") {
      var desc = Object.getOwnPropertyDescriptor(this, "prototype");
      if (desc && desc.writable === false && desc.configurable === false) return "class { }";
    }
    return text;
  };
})();
(function () {
  if (typeof structuredClone !== "function") {
    globalThis.structuredClone = function (value) {
      var seen = new Map();
      function clone(v) {
        if (v === null || (typeof v !== "object" && typeof v !== "function")) return v;
        if (typeof v === "function" || typeof v === "symbol") {
          throw new TypeError("Value could not be cloned");
        }
        if (seen.has(v)) return seen.get(v);
        if (v instanceof Date) {
          var date = new Date(v.getTime());
          seen.set(v, date);
          return date;
        }
        if (v instanceof RegExp) {
          var regexp = new RegExp(v.source, v.flags);
          seen.set(v, regexp);
          return regexp;
        }
        if (v instanceof ArrayBuffer) {
          var buffer = v.slice(0);
          seen.set(v, buffer);
          return buffer;
        }
        if (ArrayBuffer.isView(v)) {
          var view = new v.constructor(v);
          seen.set(v, view);
          return view;
        }
        if (v instanceof Map) {
          var map = new Map();
          seen.set(v, map);
          v.forEach(function (item, key) { map.set(clone(key), clone(item)); });
          return map;
        }
        if (v instanceof Set) {
          var set = new Set();
          seen.set(v, set);
          v.forEach(function (item) { set.add(clone(item)); });
          return set;
        }
        if (v instanceof Error) {
          var error = new v.constructor(v.message);
          error.name = v.name;
          seen.set(v, error);
          return error;
        }
        if (Array.isArray(v)) {
          var list = new Array(v.length);
          seen.set(v, list);
          for (var i = 0; i < v.length; i++) list[i] = clone(v[i]);
          return list;
        }
        var object = {};
        seen.set(v, object);
        var keys = Object.keys(v);
        for (var j = 0; j < keys.length; j++) object[keys[j]] = clone(v[keys[j]]);
        return object;
      }
      return clone(value);
    };
  }
  if (typeof Error.isError !== "function") {
    Error.isError = function (value) { return value instanceof Error; };
  }
  if (typeof SuppressedError !== "function") {
    function SuppressedError(error, suppressed, message) {
      var self = new Error(message === undefined ? "An error was suppressed during disposal" : String(message));
      Object.setPrototypeOf(self, SuppressedError.prototype);
      self.name = "SuppressedError";
      self.error = error;
      self.suppressed = suppressed;
      return self;
    }
    SuppressedError.prototype = Object.create(Error.prototype);
    SuppressedError.prototype.constructor = SuppressedError;
    globalThis.SuppressedError = SuppressedError;
  }
  if (typeof Array.fromAsync !== "function") {
    Array.fromAsync = function (items, mapfn, thisArg) {
      var mapper = typeof mapfn === "function" ? mapfn : function (item) { return item; };
      return Promise.resolve(items).then(function (seq) {
        return Promise.all(Array.from(seq, function (item, index) {
          return Promise.resolve(mapper.call(thisArg, item, index));
        }));
      });
    };
  }
  if (typeof ArrayBuffer.prototype.transfer !== "function") {
    ArrayBuffer.prototype.transfer = function (newLength) {
      var length = newLength === undefined ? this.byteLength : Number(newLength);
      var next = new ArrayBuffer(length);
      new Uint8Array(next).set(new Uint8Array(this, 0, Math.min(this.byteLength, length)));
      return next;
    };
  }
  var standard = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  var urlTable = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  function encode64(bytes, alphabet, omitPadding) {
    var table = alphabet === "base64url" ? urlTable : standard;
    var out = "";
    var i = 0;
    for (; i + 2 < bytes.length; i += 3) {
      var n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
      out += table[(n >> 18) & 63] + table[(n >> 12) & 63] + table[(n >> 6) & 63] + table[n & 63];
    }
    if (i < bytes.length) {
      var rest = bytes.length - i;
      var m = bytes[i] << 16;
      if (rest === 2) m |= bytes[i + 1] << 8;
      out += table[(m >> 18) & 63] + table[(m >> 12) & 63];
      if (rest === 2) out += table[(m >> 6) & 63];
      if (!omitPadding) out += rest === 1 ? "==" : "=";
    }
    return out;
  }
  function decode64(text, alphabet) {
    var table = alphabet === "base64url" ? urlTable : standard;
    var clean = String(text).replace(/[\t\n\f\r ]/g, "");
    var pad = 0;
    while (clean.endsWith("=")) { pad += 1; clean = clean.slice(0, -1); }
    var bytes = [];
    var buffer = 0;
    var bits = 0;
    for (var i = 0; i < clean.length; i++) {
      var value = table.indexOf(clean.charAt(i));
      if (value < 0) throw new SyntaxError("Invalid base64");
      buffer = (buffer << 6) | value;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes.push((buffer >> bits) & 255);
      }
    }
    if (pad) bytes = bytes.slice(0, bytes.length);
    return new Uint8Array(bytes);
  }
  if (typeof Uint8Array.prototype.toBase64 !== "function") {
    Uint8Array.prototype.toBase64 = function (options) {
      return encode64(this, options && options.alphabet, !!(options && options.omitPadding));
    };
  }
  if (typeof Uint8Array.fromBase64 !== "function") {
    Uint8Array.fromBase64 = function (string, options) {
      return decode64(string, options && options.alphabet);
    };
  }
})();
"#;

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
    object
        .function(
            NativeFunction::from_fn_ptr(console_log),
            js_string!("error"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(console_log),
            js_string!("info"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(console_log),
            js_string!("debug"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(console_log),
            js_string!("trace"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(console_log),
            js_string!("dir"),
            1,
        );
    Ok(object.build())
}

fn navigator_object(context: &mut Context) -> JsResult<JsObject> {
    let languages = JsArray::from_iter([JsValue::from(js_string!("en"))], context);
    let mime_types = plugin_array(context)?;
    let plugins = plugin_array(context)?;
    let ua_data = user_agent_data(context)?;
    let mut object = ObjectInitializer::new(context);
    object.property(
        js_string!("userAgent"),
        js_string!(mithu_net::USER_AGENT.to_string()),
        ATTR,
    );
    object.property(js_string!("platform"), js_string!("Linux x86_64"), ATTR);
    object.property(js_string!("language"), js_string!("en"), ATTR);
    object.property(js_string!("languages"), languages, ATTR);
    object.property(js_string!("doNotTrack"), JsValue::null(), ATTR);
    object.property(js_string!("cookieEnabled"), JsValue::from(true), ATTR);
    object.property(js_string!("onLine"), JsValue::from(true), ATTR);
    object.property(js_string!("mimeTypes"), mime_types, ATTR);
    object.property(js_string!("plugins"), plugins, ATTR);
    object.property(js_string!("userAgentData"), ua_data, ATTR);
    object.property(js_string!("vendor"), js_string!(""), ATTR);
    object.property(js_string!("maxTouchPoints"), JsValue::from(0), ATTR);
    object.function(
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(true))),
        js_string!("sendBeacon"),
        1,
    );
    Ok(object.build())
}

fn plugin_array(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("length"), JsValue::from(0), ATTR)
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("item"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("namedItem"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            js_string!("refresh"),
            0,
        );
    Ok(object.build())
}

fn ua_brand(context: &mut Context, brand: &str, version: &str) -> JsObject {
    ObjectInitializer::new(context)
        .property(js_string!("brand"), js_string!(brand.to_string()), ATTR)
        .property(js_string!("version"), js_string!(version.to_string()), ATTR)
        .build()
}

fn user_agent_data(context: &mut Context) -> JsResult<JsObject> {
    let brands = JsArray::from_iter(
        [
            JsValue::from(ua_brand(context, "Chromium", mithu_net::CHROME_MAJOR)),
            JsValue::from(ua_brand(context, "Mithu", "0")),
            JsValue::from(ua_brand(context, "Not)A;Brand", "24")),
        ],
        context,
    );
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("brands"), brands, ATTR)
        .property(js_string!("mobile"), JsValue::from(false), ATTR)
        .property(js_string!("platform"), js_string!("Linux"), ATTR)
        .function(
            NativeFunction::from_fn_ptr(ua_high_entropy),
            js_string!("getHighEntropyValues"),
            1,
        );
    Ok(object.build())
}

fn ua_high_entropy(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = user_agent_data(context)?;
    data.set(js_string!("architecture"), js_string!("x86"), true, context)?;
    data.set(js_string!("bitness"), js_string!("64"), true, context)?;
    data.set(js_string!("model"), js_string!(""), true, context)?;
    data.set(js_string!("platformVersion"), js_string!(""), true, context)?;
    data.set(
        js_string!("uaFullVersion"),
        js_string!(format!("{}.0.0.0", mithu_net::CHROME_MAJOR)),
        true,
        context,
    )?;
    let full = JsArray::from_iter(
        [
            JsValue::from(ua_brand(
                context,
                "Chromium",
                &format!("{}.0.0.0", mithu_net::CHROME_MAJOR),
            )),
            JsValue::from(ua_brand(context, "Mithu", mithu_net::PRODUCT_VERSION)),
            JsValue::from(ua_brand(context, "Not)A;Brand", "24.0.0.0")),
        ],
        context,
    );
    data.set(js_string!("fullVersionList"), full, true, context)?;
    Ok(JsPromise::resolve(data, context)?.into())
}

fn screen_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("width"), JsValue::from(1280), ATTR)
        .property(js_string!("height"), JsValue::from(800), ATTR)
        .property(js_string!("availWidth"), JsValue::from(1280), ATTR)
        .property(js_string!("availHeight"), JsValue::from(800), ATTR)
        .property(js_string!("colorDepth"), JsValue::from(24), ATTR)
        .property(js_string!("pixelDepth"), JsValue::from(24), ATTR)
        .property(js_string!("top"), JsValue::from(0), ATTR)
        .property(js_string!("left"), JsValue::from(0), ATTR);
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
        .accessor(js_string!("origin"), Some(origin_get), None, ATTR)
        .function(
            NativeFunction::from_fn_ptr(set_location),
            js_string!("assign"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(set_location),
            js_string!("replace"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(reload_location),
            js_string!("reload"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(get_location),
            js_string!("toString"),
            0,
        );
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

fn set_domain(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
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
    let adopted_sheets_get = native(context, "get adoptedStyleSheets", 0, adopted_get);
    let adopted_sheets_set = native(context, "set adoptedStyleSheets", 1, adopted_set);
    let scripts_get = native(context, "get scripts", 0, get_scripts);
    let ready_get = native(context, "get readyState", 0, get_ready_state);
    let base_get = native(context, "get baseURI", 0, get_location);
    let cookie_get = native(context, "get cookie", 0, get_cookie);
    let cookie_set = native(context, "set cookie", 1, set_cookie);
    let domain_get = native(context, "get domain", 0, get_hostname);
    let domain_set = native(context, "set domain", 1, set_domain);
    let fonts = font_set(context)?;
    let implementation = implementation_object(context)?;
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
            NativeFunction::from_fn_ptr(create_comment),
            js_string!("createComment"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_document_fragment),
            js_string!("createDocumentFragment"),
            0,
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
            NativeFunction::from_fn_ptr(get_elements_by_name),
            js_string!("getElementsByName"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(element_from_point),
            js_string!("elementFromPoint"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(elements_from_point),
            js_string!("elementsFromPoint"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(document_listener),
            js_string!("addEventListener"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(document_dispatch),
            js_string!("dispatchEvent"),
            1,
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
        .accessor(
            js_string!("adoptedStyleSheets"),
            Some(adopted_sheets_get),
            Some(adopted_sheets_set),
            ATTR,
        )
        .accessor(js_string!("scripts"), Some(scripts_get), None, ATTR)
        .accessor(js_string!("readyState"), Some(ready_get), None, ATTR)
        .accessor(js_string!("baseURI"), Some(base_get), None, ATTR)
        .accessor(
            js_string!("cookie"),
            Some(cookie_get),
            Some(cookie_set),
            ATTR,
        )
        .accessor(
            js_string!("domain"),
            Some(domain_get),
            Some(domain_set),
            ATTR,
        )
        .property(js_string!("fonts"), fonts, ATTR)
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
            NativeFunction::from_fn_ptr(create_event),
            js_string!("createEvent"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_tree_walker),
            js_string!("createTreeWalker"),
            1,
        )
        .property(js_string!("implementation"), implementation, ATTR)
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

pub(crate) fn adopted_get(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if let Some(object) = this.as_object() {
        let cached = object.get(js_string!("__adopted"), context)?;
        if cached.is_object() {
            return Ok(cached);
        }
    }
    Ok(JsObject::from(JsArray::from_iter(Vec::<JsValue>::new(), context)).into())
}

pub(crate) fn adopted_set(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let value = args.first().cloned().unwrap_or(JsValue::undefined());
    if let Some(object) = this.as_object() {
        let _ = object.set(js_string!("__adopted"), value.clone(), true, context);
    }
    let css = adopted_css(&value, context)?;
    apply_adopted_css(this, &css, context)?;
    Ok(value)
}

fn adopted_css(value: &JsValue, context: &mut Context) -> JsResult<String> {
    let Some(object) = value.as_object() else {
        return Ok(String::new());
    };
    let length = object
        .get(js_string!("length"), context)?
        .as_number()
        .unwrap_or(0.0) as usize;
    let mut css = String::new();
    for index in 0..length.min(32) {
        let sheet = object.get(js_string!(index.to_string()), context)?;
        let Some(sheet) = sheet.as_object() else {
            continue;
        };
        let text = sheet.get(js_string!("__css"), context)?;
        if let Some(text) = text.as_string() {
            css.push_str(&text.to_std_string_escaped());
            css.push('\n');
        }
    }
    Ok(css)
}

fn apply_adopted_css(this: &JsValue, css: &str, context: &mut Context) -> JsResult<()> {
    let host = host(context)?;
    let mut doc = host.doc.borrow_mut();
    let parent = node_of(this)
        .filter(|id| doc.nodes.get(id.0).is_some())
        .or_else(|| {
            doc.nodes.iter().enumerate().find_map(|(index, _)| {
                (doc.element_name(NodeId(index)) == Some("head")).then_some(NodeId(index))
            })
        });
    let Some(parent) = parent else {
        return Ok(());
    };
    let stale: Vec<NodeId> = doc
        .nodes
        .get(parent.0)
        .map(|node| node.children.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| doc.attr(*id, "mithu-adopted").is_some())
        .collect();
    for id in stale {
        doc.detach(id);
    }
    if !css.trim().is_empty() {
        let style = doc.create_element(parent, "style");
        doc.set_attr(style, "mithu-adopted", "1");
        doc.append_text(style, css);
    }
    doc.refresh_metadata();
    Ok(())
}

pub(crate) fn construct_style_sheet(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let object = ObjectInitializer::new(context)
        .function(
            NativeFunction::from_fn_ptr(style_replace_sync),
            js_string!("replaceSync"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(style_replace),
            js_string!("replace"),
            1,
        )
        .build();
    Ok(object.into())
}

fn style_replace_sync(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = args
        .first()
        .and_then(|value| value.as_string())
        .map(|text| text.to_std_string_escaped())
        .unwrap_or_default();
    if let Some(object) = this.as_object() {
        object.set(js_string!("__css"), js_string!(text), true, context)?;
    }
    Ok(JsValue::undefined())
}

fn style_replace(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    style_replace_sync(this, args, context)?;
    Ok(boa_engine::object::builtins::JsPromise::resolve(
        JsValue::undefined(),
        context,
    )?
    .into())
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

fn get_cookie(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Ok(host) = host(context) else {
        return Ok(js_string!("").into());
    };
    let Ok(url) = mithu_url::Url::parse(&host.location.borrow()) else {
        return Ok(js_string!("").into());
    };
    // HttpOnly cookies stay in the jar and are not part of this string.
    Ok(js_string!(mithu_net::document_cookie(&url)).into())
}

fn set_cookie(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = arg_string(args, 0, context)?;
    if let Ok(host) = host(context)
        && let Ok(url) = mithu_url::Url::parse(&host.location.borrow())
    {
        mithu_net::set_document_cookie(&url, &value);
    }
    Ok(JsValue::undefined())
}

fn create_tree_walker(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let walker = ObjectInitializer::new(context)
        .property(js_string!("currentNode"), JsValue::null(), ATTR)
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("nextNode"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("previousNode"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("firstChild"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::null())),
            js_string!("nextSibling"),
            0,
        )
        .build();
    Ok(walker.into())
}

fn implementation_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object.function(
        NativeFunction::from_fn_ptr(create_html_document),
        js_string!("createHTMLDocument"),
        0,
    );
    Ok(object.build())
}

fn create_html_document(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let html = create_element(this, &[js_string!("html").into()], context)?;
    let head = create_element(this, &[js_string!("head").into()], context)?;
    let body = create_element(this, &[js_string!("body").into()], context)?;
    append_child(&html, &[head.clone()], context)?;
    append_child(&html, &[body.clone()], context)?;
    let doc = ObjectInitializer::new(context)
        .property(js_string!("documentElement"), html, ATTR)
        .property(js_string!("head"), head, ATTR)
        .property(js_string!("body"), body, ATTR)
        .function(
            NativeFunction::from_fn_ptr(create_element),
            js_string!("createElement"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_element_ns),
            js_string!("createElementNS"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(create_text_node),
            js_string!("createTextNode"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_comment),
            js_string!("createComment"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create_document_fragment),
            js_string!("createDocumentFragment"),
            0,
        )
        .build();
    Ok(doc.into())
}

fn create_event(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind = arg_string(args, 0, context).unwrap_or_default();
    let event = ObjectInitializer::new(context)
        .property(js_string!("type"), js_string!(""), ATTR)
        .property(js_string!("defaultPrevented"), JsValue::from(false), ATTR)
        .property(js_string!("bubbles"), JsValue::from(false), ATTR)
        .property(js_string!("cancelable"), JsValue::from(false), ATTR)
        .property(js_string!("eventInterface"), js_string!(kind), ATTR)
        .function(
            NativeFunction::from_fn_ptr(event_init),
            js_string!("initEvent"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(event_init),
            js_string!("initCustomEvent"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(event_prevent),
            js_string!("preventDefault"),
            0,
        )
        .build();
    Ok(event.into())
}

fn event_init(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let kind = arg_string(args, 0, context).unwrap_or_default();
    object.set(js_string!("type"), js_string!(kind), true, context)?;
    if let Some(bubbles) = args.get(1).and_then(|value| value.as_boolean()) {
        object.set(js_string!("bubbles"), JsValue::from(bubbles), true, context)?;
    }
    if let Some(cancelable) = args.get(2).and_then(|value| value.as_boolean()) {
        object.set(
            js_string!("cancelable"),
            JsValue::from(cancelable),
            true,
            context,
        )?;
    }
    if let Some(detail) = args.get(3) {
        object.set(js_string!("detail"), detail.clone(), true, context)?;
    }
    Ok(JsValue::undefined())
}

fn event_prevent(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if let Some(object) = this.as_object() {
        object.set(
            js_string!("defaultPrevented"),
            JsValue::from(true),
            true,
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn get_ready_state(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(js_string!(host.ready_state.borrow().clone()).into())
}

fn signal_document_loaded(context: &mut Context) {
    let Some(html) = (|| {
        let host = context.get_data::<Host>().cloned()?;
        *host.ready_state.borrow_mut() = "interactive".to_string();
        find_tag(&host.doc.borrow(), "html")
    })() else {
        return;
    };
    // Blink fires DOMContentLoaded while readyState is still `interactive`,
    // then moves to `complete` and fires `load`.
    fire_node_listeners(context, html, "DOMContentLoaded");
    fire_window_listeners(context, html, "DOMContentLoaded");
    if let Some(host) = context.get_data::<Host>().cloned() {
        *host.ready_state.borrow_mut() = "complete".to_string();
    }
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
    // Template contents are detached, so the first node with this id may not
    // be the one in the document. Return the first connected match.
    let found = {
        let doc = host.doc.borrow();
        (0..doc.nodes.len())
            .map(NodeId)
            .find(|node| doc.attr(*node, "id") == Some(id.as_str()) && doc.is_connected(*node))
    };
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

fn get_elements_by_name(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    collect_named(this, context, move |doc, id| doc.attr(id, "name") == Some(name.as_str()))
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
    let tag = context
        .get_data::<Host>()
        .and_then(|host| {
            host.doc
                .borrow()
                .element_name(NodeId(id))
                .map(str::to_string)
        })
        .unwrap_or_default();
    let is_template = tag == "template";
    let sheet_get = has_sheet.then(|| native(context, "get sheet", 0, get_sheet));
    let content_get = is_template.then(|| native(context, "get content", 0, get_template_content));
    let node_value_get = native(context, "get element nodeValue", 0, get_null);
    let node_value_set = native(context, "set element nodeValue", 1, set_ignored);
    let nonce_get = native(context, "get nonce", 0, get_nonce);
    let connected_get = native(context, "get isConnected", 0, get_is_connected);
    let nonce_set = native(context, "set nonce", 1, set_nonce);
    let first_element_get = native(context, "get firstElementChild", 0, get_first_element_child);
    let attributes_get = native(context, "get attributes", 0, get_attributes);
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
            NativeFunction::from_fn_ptr(get_elements_by_name),
            js_string!("getElementsByName"),
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
            js_string!("firstElementChild"),
            Some(first_element_get),
            None,
            ATTR,
        )
        .accessor(js_string!("attributes"), Some(attributes_get), None, ATTR)
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
        .accessor(
            js_string!("nodeValue"),
            Some(node_value_get),
            Some(node_value_set),
            ATTR,
        )
        .accessor(js_string!("value"), Some(value_get), Some(value_set), ATTR)
        .accessor(
            js_string!("checked"),
            Some(checked_get),
            Some(checked_set),
            ATTR,
        )
        .accessor(js_string!("href"), Some(href_get), Some(href_set), ATTR)
        .accessor(js_string!("lang"), Some(lang_get), Some(lang_set), ATTR)
        .accessor(js_string!("nonce"), Some(nonce_get), Some(nonce_set), ATTR)
        .accessor(js_string!("isConnected"), Some(connected_get), None, ATTR);
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
    if let Some(content_get) = content_get {
        object.accessor(js_string!("content"), Some(content_get), None, ATTR);
    }
    let value = object.build();
    if tag.contains('-') {
        let delegate = ObjectInitializer::new(context)
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                js_string!("requestErrored"),
                0,
            )
            .build();
        let _ = value.set(js_string!("delegate"), delegate, false, context);
    }
    let ctor_name = JsString::from(element_interface(&tag));
    if let Ok(ctor) = context.global_object().get(ctor_name, context)
        && let Some(ctor) = ctor.as_object()
        && let Ok(proto) = ctor.get(js_string!("prototype"), context)
        && let Some(proto) = proto.as_object()
    {
        value.set_prototype(Some(proto.clone()));
    }
    apply_custom_prototype(&tag, &JsValue::from(value.clone()), context);
    let value = JsValue::from(value);
    remember_node(id, &value, context);
    value
}

fn element_interface(tag: &str) -> &'static str {
    match tag {
        "a" => "HTMLAnchorElement",
        "area" => "HTMLAreaElement",
        "audio" => "HTMLAudioElement",
        "base" => "HTMLBaseElement",
        "br" => "HTMLBRElement",
        "blockquote" | "q" => "HTMLQuoteElement",
        "caption" => "HTMLTableCaptionElement",
        "col" | "colgroup" => "HTMLTableColElement",
        "data" => "HTMLDataElement",
        "datalist" => "HTMLDataListElement",
        "del" | "ins" => "HTMLModElement",
        "details" => "HTMLDetailsElement",
        "dialog" => "HTMLDialogElement",
        "dl" => "HTMLDListElement",
        "embed" => "HTMLEmbedElement",
        "fieldset" => "HTMLFieldSetElement",
        "hr" => "HTMLHRElement",
        "legend" => "HTMLLegendElement",
        "map" => "HTMLMapElement",
        "menu" => "HTMLMenuElement",
        "meter" => "HTMLMeterElement",
        "object" => "HTMLObjectElement",
        "optgroup" => "HTMLOptGroupElement",
        "output" => "HTMLOutputElement",
        "pre" => "HTMLPreElement",
        "progress" => "HTMLProgressElement",
        "time" => "HTMLTimeElement",
        "track" => "HTMLTrackElement",
        "body" => "HTMLBodyElement",
        "button" => "HTMLButtonElement",
        "canvas" => "HTMLCanvasElement",
        "div" => "HTMLDivElement",
        "form" => "HTMLFormElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
        "head" => "HTMLHeadElement",
        "html" => "HTMLHtmlElement",
        "iframe" => "HTMLIFrameElement",
        "img" => "HTMLImageElement",
        "input" => "HTMLInputElement",
        "label" => "HTMLLabelElement",
        "li" => "HTMLLIElement",
        "link" => "HTMLLinkElement",
        "meta" => "HTMLMetaElement",
        "ol" => "HTMLOListElement",
        "option" => "HTMLOptionElement",
        "p" => "HTMLParagraphElement",
        "picture" => "HTMLPictureElement",
        "script" => "HTMLScriptElement",
        "select" => "HTMLSelectElement",
        "slot" => "HTMLSlotElement",
        "source" => "HTMLSourceElement",
        "span" => "HTMLSpanElement",
        "style" => "HTMLStyleElement",
        "table" => "HTMLTableElement",
        "tbody" | "thead" | "tfoot" => "HTMLTableSectionElement",
        "td" | "th" => "HTMLTableCellElement",
        "template" => "HTMLTemplateElement",
        "textarea" => "HTMLTextAreaElement",
        "title" => "HTMLTitleElement",
        "tr" => "HTMLTableRowElement",
        "ul" => "HTMLUListElement",
        "video" => "HTMLVideoElement",
        _ => "HTMLElement",
    }
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
    let stamp = format!("{x}:{y}:{width}:{height}");
    if let Some(object) = this.as_object() {
        let cached = object.get(js_string!("__mithuRect"), context)?;
        if let Some(cached_object) = cached.as_object() {
            let cached_stamp = cached_object
                .get(js_string!("__stamp"), context)
                .ok()
                .and_then(|value| value.as_string())
                .map(|value| value.to_std_string_escaped())
                .unwrap_or_default();
            if cached_stamp == stamp {
                return Ok(cached);
            }
        }
    }
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("x"), JsValue::from(x), ATTR)
        .property(js_string!("y"), JsValue::from(y), ATTR)
        .property(js_string!("width"), JsValue::from(width), ATTR)
        .property(js_string!("height"), JsValue::from(height), ATTR)
        .property(js_string!("left"), JsValue::from(x), ATTR)
        .property(js_string!("top"), JsValue::from(y), ATTR)
        .property(js_string!("right"), JsValue::from(x + width), ATTR)
        .property(js_string!("bottom"), JsValue::from(y + height), ATTR)
        .property(js_string!("__stamp"), js_string!(stamp), ATTR);
    let rect = JsValue::from(object.build());
    if let Some(object) = this.as_object() {
        let _ = object.set(js_string!("__mithuRect"), rect.clone(), true, context);
    }
    Ok(rect)
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
            added: Vec::new(),
            removed: Vec::new(),
        });
    Ok(())
}

fn match_media(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = arg_string(args, 0, context).unwrap_or_default();
    let global = context.global_object();
    let existing = global.get(js_string!("__mithuMql"), context)?;
    let bag = if let Some(object) = existing.as_object() {
        object.clone()
    } else {
        let object = ObjectInitializer::new(context).build();
        global.set(js_string!("__mithuMql"), object.clone(), true, context)?;
        object
    };
    let cached = bag.get(js_string!(query.clone()), context)?;
    if cached.is_object() {
        return Ok(cached);
    }
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("media"), js_string!(query.clone()), ATTR)
        .property(js_string!("matches"), JsValue::from(media_matches(&query)), ATTR)
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
    let list = JsValue::from(object.build());
    bag.set(js_string!(query), list.clone(), true, context)?;
    Ok(list)
}

/// Viewport is 1280×800. A query is answered once and then cached, so a
/// `useSyncExternalStore` snapshot keeps the same boolean.
fn media_matches(query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    if query.contains("prefers-color-scheme") && query.contains("dark") {
        return false;
    }
    if query.contains("prefers-reduced-motion") {
        return false;
    }
    if let Some(px) = media_length(&query, "min-width") {
        return 1280.0 >= px;
    }
    if let Some(px) = media_length(&query, "max-width") {
        return 1280.0 <= px;
    }
    if let Some(px) = media_length(&query, "min-height") {
        return 800.0 >= px;
    }
    if let Some(px) = media_length(&query, "max-height") {
        return 800.0 <= px;
    }
    false
}

fn media_length(query: &str, feature: &str) -> Option<f32> {
    let rest = query.split(feature).nth(1)?;
    let rest = rest.trim_start().trim_start_matches(':').trim_start();
    let digits: String = rest
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let px = digits.parse::<f32>().ok()?;
    if rest.contains("em") {
        Some(px * 16.0)
    } else {
        Some(px)
    }
}

fn get_computed_style(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().and_then(node_of).ok_or_else(type_error)?;
    let properties = computed_properties(id, context);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id: id.0 }, context);
    for (name, value) in &properties {
        object.property(js_string!(name.clone()), js_string!(value.clone()), ATTR);
    }
    object.function(
        NativeFunction::from_fn_ptr(computed_property),
        js_string!("getPropertyValue"),
        1,
    );
    Ok(object.build().into())
}

fn computed_properties(id: NodeId, context: &mut Context) -> Vec<(String, String)> {
    let Ok(host) = host(context) else {
        return Vec::new();
    };
    let doc = host.doc.borrow();
    let css = doc.stylesheets.join("\n");
    let styles = mithu_style::style_document(&doc, &css);
    let style = styles
        .get(id.0)
        .cloned()
        .unwrap_or_else(mithu_style::ComputedStyle::initial);
    let font = format_px(style.font_size);
    let display = match style.display {
        mithu_style::Display::Inline => "inline",
        mithu_style::Display::Block => "block",
        mithu_style::Display::InlineBlock => "inline-block",
        mithu_style::Display::ListItem => "list-item",
        mithu_style::Display::Flex => "flex",
        mithu_style::Display::InlineFlex => "inline-flex",
        mithu_style::Display::Grid => "grid",
        mithu_style::Display::Contents => "contents",
        mithu_style::Display::None => "none",
    };
    let float_side = match style.float {
        mithu_style::FloatSide::None => "none",
        mithu_style::FloatSide::Left => "left",
        mithu_style::FloatSide::Right => "right",
    };
    let position = match style.position {
        mithu_style::Position::Static => "static",
        mithu_style::Position::Relative => "relative",
        mithu_style::Position::Absolute => "absolute",
        mithu_style::Position::Fixed => "fixed",
        mithu_style::Position::Sticky => "sticky",
    };
    vec![
        ("fontSize".into(), font.clone()),
        ("font-size".into(), font),
        ("fontFamily".into(), style.font_family.clone()),
        ("font-family".into(), style.font_family),
        (
            "fontWeight".into(),
            if style.bold { "700" } else { "400" }.into(),
        ),
        (
            "font-weight".into(),
            if style.bold { "700" } else { "400" }.into(),
        ),
        ("display".into(), display.into()),
        ("float".into(), float_side.into()),
        ("cssFloat".into(), float_side.into()),
        ("position".into(), position.into()),
        (
            "color".into(),
            format_color(style.color.r, style.color.g, style.color.b, style.color.a),
        ),
        (
            "backgroundColor".into(),
            format_color(
                style.background.r,
                style.background.g,
                style.background.b,
                style.background.a,
            ),
        ),
        (
            "background-color".into(),
            format_color(
                style.background.r,
                style.background.g,
                style.background.b,
                style.background.a,
            ),
        ),
        ("width".into(), format_length(style.width)),
        ("height".into(), format_length(style.height)),
        ("opacity".into(), format!("{:.3}", style.opacity)),
        (
            "visibility".into(),
            if style.visible { "visible" } else { "hidden" }.into(),
        ),
        (
            "lineHeight".into(),
            match style.line_height {
                mithu_style::LineHeight::Normal => "normal".into(),
                mithu_style::LineHeight::Px(value) => format_px(value),
                mithu_style::LineHeight::Number(value) => format!("{value}"),
                mithu_style::LineHeight::Percent(value) => format!("{value}%"),
            },
        ),
    ]
}

fn format_px(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}px")
    } else {
        format!("{value}px")
    }
}

fn format_length(length: mithu_style::Length) -> String {
    match length {
        mithu_style::Length::Auto => "auto".into(),
        mithu_style::Length::Px(value) => format_px(value),
        mithu_style::Length::Percent(value) => format!("{value}%"),
        mithu_style::Length::Em(value) => format!("{value}em"),
        mithu_style::Length::Rem(value) => format!("{value}rem"),
        mithu_style::Length::Vw(value) => format!("{value}vw"),
        mithu_style::Length::Vh(value) => format!("{value}vh"),
        mithu_style::Length::Calc { percent, px, .. } => format!("calc({percent}% + {px}px)"),
    }
}

fn format_color(r: u8, g: u8, b: u8, a: u8) -> String {
    if a == 0 {
        "rgba(0, 0, 0, 0)".into()
    } else if a == 255 {
        format!("rgb({r}, {g}, {b})")
    } else {
        format!("rgba({r}, {g}, {b}, {:.3})", a as f32 / 255.0)
    }
}

fn computed_property(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(js_string!("").into());
    };
    let name = arg_string(args, 0, context).unwrap_or_default();
    let camel = kebab_to_camel(&name);
    for candidate in [name.as_str(), camel.as_str()] {
        let value = object.get(js_string!(candidate.to_string()), context)?;
        if value.is_string() {
            return Ok(value);
        }
    }
    Ok(js_string!("").into())
}

fn kebab_to_camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for ch in name.chars() {
        if ch == '-' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn queue_microtask(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let function = args.first().cloned().unwrap_or(JsValue::undefined());
    if !function
        .as_object()
        .is_some_and(|object| object.is_callable())
    {
        return Ok(JsValue::undefined());
    }
    // A fulfilled promise still queues its reaction as a microtask.
    let promise = JsPromise::resolve(JsValue::undefined(), context)?;
    let promise = JsValue::from(promise);
    if let Some(object) = promise.as_object() {
        let then = object.get(js_string!("then"), context)?;
        if let Some(then) = then.as_object() {
            let _ = then.call(&promise, &[function], context)?;
        }
    }
    Ok(JsValue::undefined())
}

fn custom_elements(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object
        .function(
            NativeFunction::from_fn_ptr(define_custom),
            js_string!("define"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(get_custom),
            js_string!("get"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(upgrade_custom),
            js_string!("upgrade"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(when_defined),
            js_string!("whenDefined"),
            1,
        );
    Ok(object.build())
}

fn custom_registry(context: &mut Context) -> JsResult<JsObject> {
    let global = context.global_object();
    let existing = global.get(js_string!("__mithuCE"), context)?;
    if let Some(object) = existing.as_object() {
        return Ok(object.clone());
    }
    let created = ObjectInitializer::new(context).build();
    context
        .global_object()
        .set(js_string!("__mithuCE"), created.clone(), false, context)?;
    Ok(created)
}

fn define_custom(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    let ctor = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let bag = custom_registry(context)?;
    bag.set(js_string!(name.clone()), ctor.clone(), true, context)?;
    if std::env::var_os("MITHU_JS_DEBUG").is_some() {
        static DEFINED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = DEFINED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 40 || name.contains("ytd-") || name.contains("app") {
            eprintln!("customElements.define {name}");
        }
    }
    resolve_when_defined(&name, &ctor, context)?;
    if let Ok(host) = host(context) {
        let ids: Vec<usize> = host
            .doc
            .borrow()
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| match &node.data {
                mithu_dom::NodeData::Element(element)
                    if element.name.eq_ignore_ascii_case(&name) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect();
        for id in ids {
            let value = element_value(id, context);
            apply_custom_prototype(&name, &value, context);
        }
    }
    Ok(JsValue::undefined())
}

fn when_defined_bag(context: &mut Context) -> JsResult<JsObject> {
    let global = context.global_object();
    let existing = global.get(js_string!("__mithuWhen"), context)?;
    if let Some(object) = existing.as_object() {
        return Ok(object.clone());
    }
    let created = ObjectInitializer::new(context).build();
    context
        .global_object()
        .set(js_string!("__mithuWhen"), created.clone(), false, context)?;
    Ok(created)
}

fn when_defined(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context).unwrap_or_default();
    let bag = custom_registry(context)?;
    let existing = bag.get(js_string!(name.clone()), context)?;
    if existing.as_object().is_some() && !existing.is_null() && !existing.is_undefined() {
        return Ok(JsPromise::resolve(existing, context)?.into());
    }
    let (promise, resolvers) = JsPromise::new_pending(context);
    let waits = when_defined_bag(context)?;
    let key = js_string!(name);
    let current = waits.get(key.clone(), context)?;
    let list = if let Some(object) = current.as_object() {
        object.clone()
    } else {
        JsObject::from(JsArray::from_iter([], context))
    };
    let length = list
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    list.set(length, resolvers.resolve, true, context)?;
    waits.set(key, list, true, context)?;
    Ok(promise.into())
}

fn resolve_when_defined(name: &str, ctor: &JsValue, context: &mut Context) -> JsResult<()> {
    let waits = when_defined_bag(context)?;
    let key = js_string!(name);
    let current = waits.get(key.clone(), context)?;
    let Some(list) = current.as_object() else {
        return Ok(());
    };
    let length = list
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    for index in 0..length {
        let resolve = list.get(index, context)?;
        if let Some(resolve) = resolve.as_object()
            && resolve.is_callable()
        {
            let _ = resolve.call(&JsValue::undefined(), &[ctor.clone()], context);
        }
    }
    let _ = waits.set(key, JsValue::undefined(), true, context);
    Ok(())
}

fn get_custom(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context).unwrap_or_default();
    let bag = custom_registry(context)?;
    bag.get(js_string!(name), context)
}

fn upgrade_custom(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = args.first().cloned().unwrap_or(JsValue::undefined());
    let Some(id) = node_of(&element) else {
        return Ok(JsValue::undefined());
    };
    let name = host(context)?
        .doc
        .borrow()
        .element_name(id)
        .unwrap_or("")
        .to_string();
    apply_custom_prototype(&name, &element, context);
    Ok(JsValue::undefined())
}

fn apply_custom_prototype(tag: &str, value: &JsValue, context: &mut Context) {
    if !tag.contains('-') {
        return;
    }
    // A constructor that creates another instance of itself would recurse
    // until the native stack aborts. Stop that chain and keep the element.
    const MAX_UPGRADE_DEPTH: u32 = 64;
    thread_local! {
        static UPGRADE_DEPTH: Cell<u32> = const { Cell::new(0) };
    }
    let depth = UPGRADE_DEPTH.with(|slot| {
        let depth = slot.get();
        slot.set(depth.saturating_add(1));
        depth
    });
    struct UpgradeGuard;
    impl Drop for UpgradeGuard {
        fn drop(&mut self) {
            UPGRADE_DEPTH.with(|slot| slot.set(slot.get().saturating_sub(1)));
        }
    }
    let _guard = UpgradeGuard;
    if depth >= MAX_UPGRADE_DEPTH {
        if std::env::var_os("MITHU_JS_DEBUG").is_some() {
            eprintln!("custom element upgrade nested too deep: {tag}");
        }
        return;
    }
    let Ok(bag) = context
        .global_object()
        .get(js_string!("__mithuCE"), context)
    else {
        return;
    };
    let Some(bag) = bag.as_object() else {
        return;
    };
    let Ok(ctor) = bag.get(js_string!(tag.to_string()), context) else {
        return;
    };
    let Some(ctor) = ctor.as_object() else {
        return;
    };
    let Ok(proto) = ctor.get(js_string!("prototype"), context) else {
        return;
    };
    if let (Some(proto), Some(object)) = (proto.as_object(), value.as_object()) {
        object.set_prototype(Some(proto.clone()));
        let upgraded = object
            .get(js_string!("__mithuUpgraded"), context)
            .ok()
            .is_some_and(|flag| flag.as_boolean() == Some(true));
        if upgraded {
            return;
        }
        let _ = object.set(
            js_string!("__mithuUpgraded"),
            JsValue::from(true),
            false,
            context,
        );
        // `super()` inside the class returns this element. A function-style
        // constructor is called with the element as `this`.
        let ctor_value = JsValue::from(ctor.clone());
        if let Some(constructor) = ctor_value.as_constructor() {
            crate::web::begin_custom_upgrade(value.clone());
            if let Err(error) = constructor.construct(&[], Some(&ctor), context)
                && std::env::var_os("MITHU_JS_DEBUG").is_some()
            {
                static ONCE: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("custom element constructor: {error}");
                }
            }
            crate::web::end_custom_upgrade();
        } else if ctor.is_callable() {
            let _ = ctor.call(value, &[], context);
        }
        let connected = node_of(value).is_some_and(|id| {
            host(context)
                .ok()
                .is_some_and(|host| host.doc.borrow().is_connected(id))
        });
        if connected {
            call_connected(value, context);
        }
    }
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
        )
        .function(
            NativeFunction::from_fn_ptr(get_property_value),
            js_string!("getPropertyValue"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(remove_property),
            js_string!("removeProperty"),
            1,
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

fn get_property_value(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context).unwrap_or_default();
    read_style(this, &name, context)
}

fn remove_property(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context).unwrap_or_default();
    let previous = read_style(this, &name, context)?;
    let _ = write_style(this, &name, &[js_string!("").into()], context);
    Ok(previous)
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
        .find(|declaration| declaration.name.eq_ignore_ascii_case(name))
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
        .find(|declaration| declaration.name.eq_ignore_ascii_case(name))
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
    let character = {
        let doc = host.doc.borrow();
        matches!(
            doc.node(id).data,
            mithu_dom::NodeData::Text(_) | mithu_dom::NodeData::Comment(_)
        )
    };
    host.doc.borrow_mut().set_text_content(id, &value);
    if character {
        note_mutation(context, id, "characterData", None)?;
    }
    Ok(JsValue::undefined())
}

fn get_html(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let doc = host.doc.borrow();
    let target = doc.template_content(id).unwrap_or(id);
    Ok(js_string!(serialize(&doc, target)).into())
}

fn set_html(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let html = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    let id = if doc.element_name(id) == Some("template") {
        ensure_template_content(&mut doc, id)
    } else {
        id
    };
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

fn get_is_connected(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Ok(host) = host(context) else {
        return Ok(JsValue::from(false));
    };
    let Some(id) = node_of(this) else {
        return Ok(JsValue::from(false));
    };
    Ok(JsValue::from(host.doc.borrow().is_connected(id)))
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

fn get_scripts(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let mut ids = Vec::new();
    let root = host.doc.borrow().root;
    collect_tags(&host.doc.borrow(), root, "script", &mut ids);
    let values = ids
        .into_iter()
        .map(|id| element_value(id.0, context))
        .collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn collect_tags(doc: &mithu_dom::Document, id: NodeId, tag: &str, out: &mut Vec<NodeId>) {
    if doc
        .element_name(id)
        .is_some_and(|name| name.eq_ignore_ascii_case(tag))
    {
        out.push(id);
    }
    for child in doc.node(id).children.clone() {
        collect_tags(doc, child, tag, out);
    }
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

fn replace_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let new_node = args.first().cloned().unwrap_or(JsValue::null());
    let old_node = args.get(1).cloned().unwrap_or(JsValue::null());
    insert_before_child(this, &[new_node, old_node.clone()], context)?;
    remove_child(this, &[old_node.clone()], context)?;
    Ok(old_node)
}

fn toggle_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    let present = host.doc.borrow().attr(id, &name).is_some();
    if present {
        host.doc.borrow_mut().remove_attr(id, &name);
        Ok(JsValue::from(false))
    } else {
        let value = args
            .get(1)
            .and_then(|value| arg_string(std::slice::from_ref(value), 0, context).ok())
            .unwrap_or_default();
        host.doc.borrow_mut().set_attr(id, &name, &value);
        Ok(JsValue::from(true))
    }
}

fn attribute_ns(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args.get(1).cloned().unwrap_or(JsValue::undefined());
    get_attribute(this, &[name], context)
}

fn set_attribute_ns(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let value = args.get(2).cloned().unwrap_or(JsValue::undefined());
    set_attribute(this, &[name, value], context)
}

fn remove_attribute_ns(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = args.get(1).cloned().unwrap_or(JsValue::undefined());
    remove_attribute(this, &[name], context)
}

fn insert_adjacent_element(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let position = arg_string(args, 0, context)?.to_ascii_lowercase();
    let node = args.get(1).cloned().unwrap_or(JsValue::null());
    if node_of(&node).is_none() {
        return Ok(JsValue::null());
    }
    match position.as_str() {
        "beforeend" | "afterbegin" => {
            if position == "afterbegin" {
                prepend_nodes(this, &[node.clone()], context)?;
            } else {
                append_nodes(this, &[node.clone()], context)?;
            }
        }
        "beforebegin" | "afterend" => {
            let host = host(context)?;
            let id = node_of(this).ok_or_else(type_error)?;
            let parent = host.doc.borrow().parent_element(id);
            let Some(parent) = parent else {
                return Ok(JsValue::null());
            };
            let parent_value = element_value(parent.0, context);
            if position == "beforebegin" {
                insert_before_child(&parent_value, &[node.clone(), this.clone()], context)?;
            } else {
                let next = host.doc.borrow().next_element_sibling(id);
                match next {
                    Some(next) => {
                        let next_value = element_value(next.0, context);
                        insert_before_child(&parent_value, &[node.clone(), next_value], context)?;
                    }
                    None => {
                        append_child(&parent_value, &[node.clone()], context)?;
                    }
                }
            }
        }
        _ => return Ok(JsValue::null()),
    }
    Ok(node)
}

fn remove_self(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let parent = host.doc.borrow().parent_element(id);
    if let Some(parent) = parent {
        let parent_value = element_value(parent.0, context);
        let _ = remove_child(&parent_value, &[this.clone()], context);
    }
    Ok(JsValue::undefined())
}

fn before_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    for arg in args {
        insert_adjacent_element(
            this,
            &[js_string!("beforebegin").into(), arg.clone()],
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn after_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    for arg in args.iter().rev() {
        insert_adjacent_element(this, &[js_string!("afterend").into(), arg.clone()], context)?;
    }
    Ok(JsValue::undefined())
}

fn import_node(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node = args.first().cloned().unwrap_or(JsValue::null());
    clone_node(&node, &[JsValue::from(true)], context)
}

pub(crate) fn attach_shadow(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let mode = args
        .first()
        .and_then(|value| value.as_object())
        .and_then(|object| object.get(js_string!("mode"), context).ok())
        .and_then(|value| value.as_string().map(|text| text.to_std_string_escaped()))
        .unwrap_or_else(|| "open".to_string());
    if std::env::var_os("MITHU_JS_DEBUG").is_some() {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 4 {
            let tag = node_of(this)
                .and_then(|id| {
                    host(context)
                        .ok()
                        .and_then(|host| host.doc.borrow().element_name(id).map(str::to_string))
                })
                .unwrap_or_default();
            eprintln!("attachShadow {n} {tag}");
        }
    }
    let root_id = {
        let host = host(context)?;
        host.doc
            .borrow_mut()
            .create_element_detached("mithu-shadow")
    };
    let root = element_value(root_id.0, context);
    if let Some(object) = root.as_object() {
        object.set(js_string!("mode"), js_string!(mode.clone()), true, context)?;
        object.set(js_string!("host"), this.clone(), true, context)?;
        let adopted_get_fn = native(context, "get adoptedStyleSheets", 0, adopted_get);
        let adopted_set_fn = native(context, "set adoptedStyleSheets", 1, adopted_set);
        let _ = define_accessor(
            &object,
            context,
            "adoptedStyleSheets",
            adopted_get_fn,
            Some(adopted_set_fn),
        );
    }
    if mode != "closed" {
        if let Some(object) = this.as_object() {
            object.set(js_string!("shadowRoot"), root.clone(), true, context)?;
        }
    }
    if let Some(host_id) = node_of(this) {
        let host = host(context)?;
        let mut doc = host.doc.borrow_mut();
        doc.set_attr(root_id, "mithu-host", &host_id.0.to_string());
        doc.set_attr(host_id, "mithu-shadow", &root_id.0.to_string());
    }
    Ok(root)
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

fn execute_inserted_script(id: NodeId, context: &mut Context) {
    let Some(host) = context.get_data::<Host>().cloned() else {
        return;
    };
    let (module, src, inline) = {
        let doc = host.doc.borrow();
        if doc.element_name(id) != Some("script") || !doc.is_connected(id) {
            return;
        }
        let kind = doc
            .attr(id, "type")
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let javascript = kind.is_empty()
            || kind == "module"
            || matches!(
                kind.as_str(),
                "text/javascript"
                    | "application/javascript"
                    | "text/ecmascript"
                    | "application/ecmascript"
            );
        if !javascript || doc.attr(id, "nomodule").is_some() {
            return;
        }
        let src = doc
            .attr(id, "src")
            .map(str::to_string)
            .filter(|value| !value.is_empty());
        let inline = if src.is_none() {
            doc.text_content(id)
        } else {
            String::new()
        };
        (kind == "module", src, inline)
    };
    if host.ran_scripts.borrow().contains(&id.0) {
        return;
    }
    if src.is_none() && inline.trim().is_empty() {
        return;
    }
    host.ran_scripts.borrow_mut().insert(id.0);
    let code = if let Some(src) = src.as_deref() {
        let base = host.location.borrow().clone();
        let url = resolve_request(&base, src)
            .map(|url| url.to_display())
            .unwrap_or_else(|_| src.to_string());
        match (host.loader.fetch.borrow_mut())(&url) {
            Some(code) => code,
            None => {
                host.log
                    .borrow_mut()
                    .push(format!("JavaScript error: script not found: {url}"));
                return;
            }
        }
    } else {
        inline
    };
    let path = src.unwrap_or_else(|| format!("inserted-{}.js", id.0));
    let previous = host.current_script.replace(Some(id));
    remember_source(&host, &path, &code);
    let result = if module {
        run_module(context, &code, &path)
    } else {
        let source_path = PathBuf::from(&path);
        match Script::parse(
            Source::from_bytes(code.as_str()).with_path(&source_path),
            None,
            context,
        ) {
            Ok(script) => script.evaluate(context).map(|_| ()),
            Err(error) => Err(error),
        }
    };
    if let Err(error) = result {
        note_error(context, &error);
    } else if let Err(error) = context.run_jobs() {
        note_error(context, &error);
    }
    *host.current_script.borrow_mut() = previous;
    fire_node_listeners(context, id, "load");
}

fn append_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    if let Some(holder) = fragment_host_id(&child_value, context)? {
        let children = host.doc.borrow().node(NodeId(holder)).children.clone();
        for child in children {
            host.doc.borrow_mut().append_child(parent, child);
            execute_inserted_script(child, context);
        }
        return Ok(child_value);
    }
    let child = node_of(&child_value).ok_or_else(type_error)?;
    host.doc.borrow_mut().append_child(parent, child);
    notify_connected(child, context);
    execute_inserted_script(child, context);
    Ok(child_value)
}

fn call_connected(value: &JsValue, context: &mut Context) {
    let Some(object) = value.as_object() else {
        return;
    };
    let done = object
        .get(js_string!("__mithuConnected"), context)
        .ok()
        .is_some_and(|flag| flag.as_boolean() == Some(true));
    if done {
        return;
    }
    let _ = object.set(
        js_string!("__mithuConnected"),
        JsValue::from(true),
        false,
        context,
    );
    if let Ok(callback) = object.get(js_string!("connectedCallback"), context)
        && let Some(callback) = callback.as_object()
        && callback.is_callable()
    {
        let _ = callback.call(value, &[], context);
    }
}

fn notify_connected(id: NodeId, context: &mut Context) {
    let tag = host(context)
        .ok()
        .and_then(|host| host.doc.borrow().element_name(id).map(str::to_string));
    if tag.as_deref().is_some_and(|name| name.contains('-')) {
        call_connected(&element_value(id.0, context), context);
    }
    let children = host(context)
        .map(|host| host.doc.borrow().node(id).children.clone())
        .unwrap_or_default();
    for child in children {
        notify_connected(child, context);
    }
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

fn settle_fonts(context: &mut Context) {
    let Ok(resolver) = context
        .global_object()
        .get(js_string!("__mithuFontsReady"), context)
    else {
        return;
    };
    if !resolver.is_callable() {
        return;
    }
    let _ = context.global_object().set(
        js_string!("__mithuFontsReady"),
        JsValue::undefined(),
        true,
        context,
    );
    if let Some(function) = resolver.as_object() {
        let _ = function.call(&JsValue::undefined(), &[JsValue::undefined()], context);
    }
    if let Ok(document) = context.global_object().get(js_string!("document"), context)
        && let Some(document) = document.as_object()
        && let Ok(fonts) = document.get(js_string!("fonts"), context)
        && let Some(fonts) = fonts.as_object()
    {
        let _ = fonts.set(js_string!("status"), js_string!("loaded"), true, context);
    }
}

fn run_due_timers(context: &mut Context, ms: u64) -> JsResult<bool> {
    settle_fonts(context);
    let host = host(context)?;
    {
        let mut clock = host.clock.borrow_mut();
        clock.now = clock.now.saturating_add(ms);
    }
    let mut ran = false;
    // One-shot tasks (React, rAF) need to drain. A repeating timer must not
    // occupy every slot in this pass, or those tasks never run.
    let mut fired_repeat = Vec::new();
    for _ in 0..2_000 {
        let next = {
            let clock = host.clock.borrow();
            clock
                .queue
                .iter()
                .filter(|timer| timer.due <= clock.now && !fired_repeat.contains(&timer.id))
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
        if repeat.is_some() {
            fired_repeat.push(id);
        }
        let item = timer_value(context, id)?;
        match kind {
            1 => {
                if let Some(function) = item.as_object().filter(|object| object.is_callable()) {
                    let deadline = idle_deadline(context)?;
                    function.call(&JsValue::undefined(), &[deadline], context)?;
                }
            }
            2 => crate::web::finish_xhr(&item, context)?,
            3 => deliver_macrotask_message(&item, context)?,
            4 => crate::web::deliver_layout_observer(&item, context)?,
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
    if signal_aborted(args, context)? {
        return reject_promise(context, "The operation was aborted.");
    }
    let spec = fetch_spec(args, context)?;
    if spec.is_empty() {
        return reject_promise(context, "fetch requires a url");
    }
    if std::env::var_os("MITHU_JS_DEBUG").is_some() {
        eprintln!("fetch {spec}");
    }
    let host = host(context)?;
    let location = host.location.borrow().clone();
    let extra = fetch_headers(args, context)?;
    let extra_refs: Vec<(&str, &str)> = extra
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let method = fetch_method(args, context);
    let body_bytes = fetch_body(args, context)?;
    let content_type = extra.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            .then_some(value.clone())
    });
    let outcome = resolve_request(&location, &spec).and_then(|url| {
        let initiator = mithu_url::Url::parse(&location)
            .ok()
            .map(|page| page.host)
            .filter(|host| !host.is_empty());
        let page = mithu_url::Url::parse(&location).ok();
        let origin = page.as_ref().map(serialized_origin);
        let response = mithu_net::send(
            &method,
            &url,
            content_type.as_deref(),
            &body_bytes,
            initiator.as_deref(),
            origin.as_deref(),
            &extra_refs,
        );
        let mode = fetch_mode(args, context);
        response
            .map_err(|error| error.to_string())
            .and_then(|response| {
                if let Some(page) = page.as_ref()
                    && cross_origin(page, &response.url)
                    && mode != "no-cors"
                    && !cors_allows(page, response.header("access-control-allow-origin"))
                {
                    return Err(format!("cross-origin fetch was blocked ({})", response.url));
                }
                if mode == "no-cors"
                    && page
                        .as_ref()
                        .is_some_and(|page| cross_origin(page, &response.url))
                {
                    return Ok((
                        0,
                        false,
                        response.url.to_string(),
                        String::new(),
                        String::new(),
                    ));
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
    page.scheme != target.scheme
        || !page.host.eq_ignore_ascii_case(&target.host)
        || page.effective_port() != target.effective_port()
}

/// Chromium's serialized origin: scheme, host, and a port only when it is not
/// the scheme's default.
pub(crate) fn serialized_origin(url: &mithu_url::Url) -> String {
    match url.port {
        Some(port) if mithu_url::Url::default_port(&url.scheme) != Some(port) => {
            format!("{}://{}:{port}", url.scheme, url.host)
        }
        _ => format!("{}://{}", url.scheme, url.host),
    }
}

pub(crate) fn cors_allows(page: &mithu_url::Url, allow: Option<&str>) -> bool {
    let Some(allow) = allow.map(str::trim) else {
        return false;
    };
    if allow == "*" {
        return true;
    }
    allow == serialized_origin(page)
}

fn fetch_headers(args: &[JsValue], context: &mut Context) -> JsResult<Vec<(String, String)>> {
    let Some(init) = args.get(1).and_then(JsValue::as_object) else {
        return Ok(Vec::new());
    };
    let headers = init.get(js_string!("headers"), context)?;
    let Some(object) = headers.as_object() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for name in ["accept", "Accept", "content-type", "Content-Type", "authorization", "Authorization"]
    {
        let value = object.get(js_string!(name), context)?;
        if value.is_null() || value.is_undefined() {
            continue;
        }
        let value = value.to_string(context)?.to_std_string_lossy();
        if value.is_empty() {
            continue;
        }
        out.push((name.to_string(), value));
    }
    Ok(out)
}

fn fetch_method(args: &[JsValue], context: &mut Context) -> String {
    for candidate in [args.get(1), args.first()] {
        let Some(object) = candidate.and_then(JsValue::as_object) else {
            continue;
        };
        let Ok(method) = object.get(js_string!("method"), context) else {
            continue;
        };
        if let Some(text) = method.as_string() {
            let method = text.to_std_string_lossy();
            if !method.is_empty() {
                return method.to_ascii_uppercase();
            }
        }
    }
    "GET".to_string()
}

fn fetch_body(args: &[JsValue], context: &mut Context) -> JsResult<Vec<u8>> {
    for candidate in [args.get(1), args.first()] {
        let Some(object) = candidate.and_then(JsValue::as_object) else {
            continue;
        };
        let body = object.get(js_string!("body"), context)?;
        if body.is_undefined() || body.is_null() {
            continue;
        }
        if let Some(object) = body.as_object()
            && let Ok(text) = object.get(js_string!("_body"), context)
            && text.is_string()
        {
            return Ok(text.to_string(context)?.to_std_string_escaped().into_bytes());
        }
        if let Some(object) = body.as_object()
            && let Ok(text) = object.get(js_string!("_text"), context)
            && text.is_string()
        {
            return Ok(text.to_string(context)?.to_std_string_escaped().into_bytes());
        }
        return Ok(body.to_string(context)?.to_std_string_escaped().into_bytes());
    }
    Ok(Vec::new())
}

fn fetch_mode(args: &[JsValue], context: &mut Context) -> String {
    let Some(init) = args.get(1).and_then(JsValue::as_object) else {
        return "cors".to_string();
    };
    init.get(js_string!("mode"), context)
        .ok()
        .and_then(|value| value.as_string())
        .map(|value| value.to_std_string_lossy())
        .unwrap_or_else(|| "cors".to_string())
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

fn signal_aborted(args: &[JsValue], context: &mut Context) -> JsResult<bool> {
    for candidate in [args.get(1), args.first()] {
        let Some(object) = candidate.and_then(JsValue::as_object) else {
            continue;
        };
        let signal = object.get(js_string!("signal"), context)?;
        let Some(signal) = signal.as_object() else {
            continue;
        };
        return Ok(signal
            .get(js_string!("aborted"), context)?
            .as_boolean()
            .unwrap_or(false));
    }
    Ok(false)
}

fn font_set(context: &mut Context) -> JsResult<JsObject> {
    // Stay pending until the page clock moves. Resolving during the current
    // task lets a font callback change the document before React hydrates it.
    let (ready, resolvers) = JsPromise::new_pending(context);
    context.register_global_property(
        js_string!("__mithuFontsReady"),
        resolvers.resolve,
        Attribute::all(),
    )?;
    let mut fonts = ObjectInitializer::new(context);
    fonts
        .property(js_string!("status"), js_string!("loading"), ATTR)
        .property(js_string!("ready"), ready, ATTR)
        .property(js_string!("size"), JsValue::from(0), ATTR)
        .function(
            NativeFunction::from_fn_ptr(fonts_check),
            js_string!("check"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fonts_load),
            js_string!("load"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fonts_listen),
            js_string!("add"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fonts_listen),
            js_string!("addEventListener"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(fonts_listen),
            js_string!("forEach"),
            1,
        );
    Ok(fonts.build())
}

fn fonts_check(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    // The engine can draw the families it has already loaded, and a missing
    // face falls back to the system font, so a critical-font check succeeds.
    Ok(JsValue::from(true))
}

fn fonts_load(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let spec = arg_string(args, 0, context).unwrap_or_default();
    let face = ObjectInitializer::new(context)
        .property(js_string!("family"), js_string!(spec), ATTR)
        .property(js_string!("status"), js_string!("loaded"), ATTR)
        .property(js_string!("weight"), js_string!("normal"), ATTR)
        .function(
            NativeFunction::from_fn_ptr(font_face_ready),
            js_string!("load"),
            0,
        )
        .build();
    let faces = JsArray::from_iter([JsValue::from(face)], context);
    Ok(JsPromise::resolve(faces, context)?.into())
}

fn font_face_ready(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsPromise::resolve(this.clone(), context)?.into())
}

fn fonts_listen(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn ensure_template_content(doc: &mut mithu_dom::Document, template: NodeId) -> NodeId {
    if let Some(fragment) = doc.template_content(template) {
        return fragment;
    }
    let fragment = doc.create_element_detached("mithu-fragment");
    doc.template_contents.push((template, fragment));
    fragment
}

fn get_template_content(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if let Some(object) = this.as_object() {
        let cached = object.get(js_string!("__content"), context)?;
        if cached.is_object() {
            return Ok(cached);
        }
    }
    let id = node_of(this).ok_or_else(type_error)?;
    let fragment_id = {
        let host = host(context)?;
        let mut doc = host.doc.borrow_mut();
        ensure_template_content(&mut doc, id)
    };
    let fragment = fragment_value(fragment_id.0, context)?;
    if let Some(object) = this.as_object() {
        object.set(js_string!("__content"), fragment.clone(), true, context)?;
    }
    Ok(fragment)
}

fn fragment_value(host_id: usize, context: &mut Context) -> JsResult<JsValue> {
    let nodes_get = native(context, "get fragment childNodes", 0, fragment_child_nodes);
    let children_get = native(context, "get fragment children", 0, fragment_children);
    let first_get = native(context, "get fragment firstChild", 0, fragment_first_child);
    let html_get = native(context, "get fragment innerHTML", 0, fragment_html_get);
    let html_set = native(context, "set fragment innerHTML", 1, fragment_html_set);
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("__host"), JsValue::from(host_id as u32), ATTR)
        .property(js_string!("nodeType"), JsValue::from(11), ATTR)
        .property(
            js_string!("nodeName"),
            js_string!("#document-fragment"),
            ATTR,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_append),
            js_string!("appendChild"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_query),
            js_string!("querySelector"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_query_all),
            js_string!("querySelectorAll"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_clone),
            js_string!("cloneNode"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_insert_before),
            js_string!("insertBefore"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_remove_child),
            js_string!("removeChild"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(fragment_replace_child),
            js_string!("replaceChild"),
            2,
        )
        .accessor(js_string!("childNodes"), Some(nodes_get), None, ATTR)
        .accessor(js_string!("children"), Some(children_get), None, ATTR)
        .accessor(js_string!("firstChild"), Some(first_get), None, ATTR)
        .accessor(
            js_string!("innerHTML"),
            Some(html_get),
            Some(html_set),
            ATTR,
        );
    Ok(object.build().into())
}

fn fragment_element(this: &JsValue, context: &mut Context) -> JsResult<JsValue> {
    let id = fragment_host_id(this, context)?.unwrap_or(0);
    Ok(element_value(id, context))
}

fn fragment_host_id(value: &JsValue, context: &mut Context) -> JsResult<Option<usize>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let kind = object.get(js_string!("nodeType"), context)?;
    if kind.as_number() != Some(11.0) {
        return Ok(None);
    }
    let id = object
        .get(js_string!("__host"), context)?
        .as_number()
        .unwrap_or(0.0) as usize;
    Ok(Some(id))
}

fn fragment_child_nodes(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    get_child_nodes(&element, args, context)
}

fn fragment_children(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    get_children(&element, args, context)
}

fn fragment_first_child(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    get_first_child(&element, args, context)
}

fn fragment_html_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    get_html(&element, args, context)
}

fn fragment_html_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    set_html(&element, args, context)
}

fn fragment_append(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    append_child(&element, args, context)
}

fn fragment_insert_before(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    insert_before_child(&element, args, context)
}

fn fragment_remove_child(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    remove_child(&element, args, context)
}

fn fragment_replace_child(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    replace_child(&element, args, context)
}

fn fragment_query(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    query_selector(&element, args, context)
}

fn fragment_query_all(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let element = fragment_element(this, context)?;
    query_selector_all(&element, args, context)
}

fn fragment_clone(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = fragment_host_id(this, context)?.unwrap_or(0);
    let html = {
        let host = host(context)?;
        serialize(&host.doc.borrow(), NodeId(id))
    };
    let holder = {
        let host = host(context)?;
        host.doc.borrow_mut().create_element_detached("div")
    };
    let element = element_value(holder.0, context);
    set_html(&element, &[js_string!(html).into()], context)?;
    fragment_value(holder.0, context)
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
    if let Ok(ctor) = context.global_object().get(js_string!("Response"), context)
        && let Some(ctor) = ctor.as_object().filter(|object| object.is_constructor())
    {
        let status_text = if ok { "OK" } else { "" };
        let init = ObjectInitializer::new(context)
            .property(js_string!("status"), JsValue::from(i32::from(status)), ATTR)
            .property(js_string!("statusText"), js_string!(status_text), ATTR)
            .property(js_string!("url"), js_string!(url.clone()), ATTR)
            .property(js_string!("headers"), js_string!(headers.clone()), ATTR)
            .build();
        if let Ok(value) = ctor.construct(
            &[js_string!(body.clone()).into(), JsValue::from(init)],
            None,
            context,
        ) {
            return value.into();
        }
    }
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
    let timing = performance_timing(context)?;
    let mut object = ObjectInitializer::new(context);
    object
        .property(js_string!("timing"), timing, ATTR)
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

fn performance_timing(context: &mut Context) -> JsResult<JsObject> {
    let now = host(context)?.clock.borrow().now as f64;
    let mut object = ObjectInitializer::new(context);
    for name in [
        "navigationStart",
        "unloadEventStart",
        "unloadEventEnd",
        "redirectStart",
        "redirectEnd",
        "fetchStart",
        "domainLookupStart",
        "domainLookupEnd",
        "connectStart",
        "connectEnd",
        "secureConnectionStart",
        "requestStart",
        "responseStart",
        "responseEnd",
        "domLoading",
        "domInteractive",
        "domContentLoadedEventStart",
        "domContentLoadedEventEnd",
        "domComplete",
        "loadEventStart",
        "loadEventEnd",
    ] {
        object.property(js_string!(name.to_string()), JsValue::from(now), ATTR);
    }
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
    let clock = host.clock.borrow();
    let elapsed = clock.wall.elapsed().as_secs_f64() * 1000.0;
    Ok(JsValue::from(clock.now as f64 + elapsed))
}

fn window_post_message(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = crate::web::clone_message_value(args.first().unwrap_or(&JsValue::undefined()), context)?;
    let event = ObjectInitializer::new(context)
        .property(js_string!("data"), data, ATTR)
        .property(js_string!("type"), js_string!("message"), ATTR)
        .property(js_string!("__window"), JsValue::from(true), ATTR)
        .build();
    schedule_callback(context, JsValue::from(event), 3, 0)?;
    Ok(JsValue::undefined())
}

fn deliver_macrotask_message(event: &JsValue, context: &mut Context) -> JsResult<()> {
    let window_message = event
        .as_object()
        .and_then(|object| object.get(js_string!("__window"), context).ok())
        .and_then(|value| value.as_boolean())
        .unwrap_or(false);
    if !window_message {
        return crate::web::deliver_port_message(event, context);
    }
    let functions = read_listeners(context, "win:message").unwrap_or_default();
    let this = JsValue::from(context.global_object().clone());
    for function in functions {
        function.call(&this, &[event.clone()], context)?;
    }
    let onmessage = context
        .global_object()
        .get(js_string!("onmessage"), context)?;
    if let Some(function) = onmessage.as_object().filter(|object| object.is_callable()) {
        function.call(&this, &[event.clone()], context)?;
    }
    Ok(())
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

fn document_dispatch(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let Some(html) = find_tag(&host.doc.borrow(), "html") else {
        return Ok(JsValue::from(true));
    };
    let element = element_value(html.0, context);
    dispatch_event(&element, args, context)
}

fn window_dispatch(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let event_name = event_type(args.first(), context)?;
    let host = host(context)?;
    if let Some(html) = find_tag(&host.doc.borrow(), "html") {
        fire_window_listeners(context, html, &event_name);
    }
    Ok(JsValue::from(true))
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

fn reload_location(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let current = host.location.borrow().clone();
    *host.navigation.borrow_mut() = Some(current);
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

fn create_comment(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = arg_string(args, 0, context).unwrap_or_default();
    let id = host.doc.borrow_mut().create_comment_detached(&text);
    Ok(node_value(id.0, context))
}

fn create_document_fragment(
    _this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = host.doc.borrow_mut().create_element_detached("mithu-fragment");
    fragment_value(id.0, context)
}

fn element_from_point(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let hits = elements_under_point(args, context)?;
    Ok(hits.into_iter().next().unwrap_or(JsValue::null()))
}

fn elements_from_point(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let hits = elements_under_point(args, context)?;
    Ok(JsArray::from_iter(hits, context).into())
}

/// Deepest laid-out element first. GPT's viewability check calls this.
fn elements_under_point(args: &[JsValue], context: &mut Context) -> JsResult<Vec<JsValue>> {
    let x = args
        .first()
        .map(|value| value.to_number(context))
        .transpose()?
        .unwrap_or(f64::NAN);
    let y = args
        .get(1)
        .map(|value| value.to_number(context))
        .transpose()?
        .unwrap_or(f64::NAN);
    if !x.is_finite() || !y.is_finite() {
        return Ok(Vec::new());
    }
    let host = host(context)?;
    let scroll = f64::from(host.scroll.get());
    let geometry = host.geometry.borrow().clone();
    let mut hits: Vec<(f32, usize)> = geometry
        .into_iter()
        .filter(|(_, metrics)| {
            let left = f64::from(metrics.x);
            let top = f64::from(metrics.y) - scroll;
            let right = left + f64::from(metrics.width);
            let bottom = top + f64::from(metrics.height);
            metrics.width > 0.0
                && metrics.height > 0.0
                && x >= left
                && x < right
                && y >= top
                && y < bottom
        })
        .map(|(id, metrics)| (metrics.width * metrics.height, id))
        .collect();
    hits.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut values = Vec::new();
    for (_, id) in hits {
        if host.doc.borrow().element_name(NodeId(id)).is_some() {
            values.push(element_value(id, context));
        }
    }
    Ok(values)
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
    let value_get = native(context, "get nodeValue", 0, get_text);
    let value_set = native(context, "set nodeValue", 1, set_text);
    let parent_get = native(context, "get parentNode", 0, get_parent_node);
    let next_get = native(context, "get nextSibling", 0, get_next_node);
    let previous_get = native(context, "get previousSibling", 0, get_previous_node);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("nodeType"), Some(type_get), None, ATTR)
        .accessor(js_string!("nodeName"), Some(name_get), None, ATTR)
        .accessor(js_string!("data"), Some(data_get), Some(data_set), ATTR)
        .accessor(
            js_string!("nodeValue"),
            Some(value_get),
            Some(value_set),
            ATTR,
        )
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
    let object = object.build();
    attach_node_prototype(&object, context);
    let value = JsValue::from(object);
    remember_node(id, &value, context);
    value
}

fn attach_node_prototype(object: &boa_engine::JsObject, context: &mut Context) {
    let Ok(ctor) = context.global_object().get(js_string!("Node"), context) else {
        return;
    };
    let Some(ctor) = ctor.as_object() else {
        return;
    };
    let Ok(proto) = ctor.get(js_string!("prototype"), context) else {
        return;
    };
    if let Some(proto) = proto.as_object() {
        object.set_prototype(Some(proto.clone()));
    }
}

fn is_equal_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(id) = node_of(this) else {
        return Ok(JsValue::from(false));
    };
    let Some(other) = args.first().and_then(node_of) else {
        return Ok(JsValue::from(false));
    };
    let host = host(context)?;
    Ok(JsValue::from(host.doc.borrow().nodes_equal(id, other)))
}

fn is_same_node(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this);
    let other = args.first().and_then(node_of);
    Ok(JsValue::from(id.is_some() && id == other))
}

fn clone_node(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = node_of(this).ok_or_else(type_error)?;
    let deep = args
        .first()
        .is_some_and(|value| value.as_boolean() == Some(true) || value.to_boolean());
    let cloned = {
        let host = host(context)?;
        host.doc.borrow_mut().clone_node(id, deep)
    };
    Ok(node_value(cloned.0, context))
}

fn get_first_element_child(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(id) = node_of(this) else {
        return Ok(JsValue::null());
    };
    let child = {
        let host = host(context)?;
        let doc = host.doc.borrow();
        doc.node(id)
            .children
            .iter()
            .copied()
            .find(|child| doc.element_name(*child).is_some())
    };
    Ok(match child {
        Some(child) => element_value(child.0, context),
        None => JsValue::null(),
    })
}

fn get_attributes(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(id) = node_of(this) else {
        return Ok(JsArray::from_iter([], context).into());
    };
    let pairs = {
        let host = host(context)?;
        let doc = host.doc.borrow();
        match &doc.node(id).data {
            NodeData::Element(element) => element.attrs.clone(),
            _ => Vec::new(),
        }
    };
    let mut values = Vec::new();
    for (name, value) in &pairs {
        let attr = ObjectInitializer::new(context)
            .property(js_string!("name"), js_string!(name.clone()), ATTR)
            .property(js_string!("value"), js_string!(value.clone()), ATTR)
            .property(js_string!("nodeName"), js_string!(name.clone()), ATTR)
            .build();
        values.push(JsValue::from(attr));
    }
    let list = JsValue::from(JsArray::from_iter(values, context));
    if let Some(object) = list.as_object() {
        for (name, value) in &pairs {
            let attr = ObjectInitializer::new(context)
                .property(js_string!("name"), js_string!(name.clone()), ATTR)
                .property(js_string!("value"), js_string!(value.clone()), ATTR)
                .build();
            object.set(js_string!(name.clone()), attr, true, context)?;
        }
    }
    Ok(list)
}

fn get_nonce(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    attr_value(this, "nonce", context)
}

fn set_nonce(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    write_attr(this, "nonce", args, context)
}

fn get_null(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::null())
}

fn set_ignored(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
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
    let written = write_attr(this, "src", args, context)?;
    if let Some(id) = node_of(this) {
        execute_inserted_script(id, context);
    }
    Ok(written)
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
    let before = args.get(1).and_then(node_of);
    if let Some(holder) = fragment_host_id(&child_value, context)? {
        let children = host.doc.borrow().node(NodeId(holder)).children.clone();
        let mut reference = before;
        for child in children.into_iter().rev() {
            host.doc
                .borrow_mut()
                .insert_before(parent, child, reference);
            execute_inserted_script(child, context);
            reference = Some(child);
        }
        return Ok(child_value);
    }
    let child = node_of(&child_value).ok_or_else(type_error)?;
    host.doc.borrow_mut().insert_before(parent, child, before);
    notify_connected(child, context);
    execute_inserted_script(child, context);
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
        NodeData::Element(element) if element.name == "mithu-fragment" => {
            for child in &doc.node(id).children {
                write_html(doc, *child, out);
            }
        }
        NodeData::Element(element) => {
            let void = matches!(
                element.name.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "source"
                    | "track"
                    | "wbr"
            );
            out.push('<');
            out.push_str(&element.name);
            for (name, value) in &element.attrs {
                out.push(' ');
                out.push_str(name);
                if !value.is_empty() {
                    out.push_str("=\"");
                    out.push_str(&escape(value));
                    out.push('"');
                }
            }
            out.push('>');
            if void {
                return;
            }
            let children = if element.name == "template" {
                doc.template_content(id)
                    .map(|fragment| doc.node(fragment).children.clone())
                    .unwrap_or_default()
            } else {
                doc.node(id).children.clone()
            };
            for child in children {
                write_html(doc, child, out);
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
        let raf = doc.attr(body, "data-raf").unwrap_or("");
        assert!(
            raf.parse::<f64>().is_ok_and(|value| value >= 0.0),
            "{raf}"
        );
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
    fn a_readable_stream_tees_and_reads_enqueued_chunks() {
        let doc = parse_html(
            r#"<html><body><script>
              var stream = new ReadableStream({
                start: function (controller) {
                  controller.enqueue("a");
                  controller.enqueue("b");
                  controller.close();
                }
              });
              var ignored = new ReadableStream();
              if (typeof ignored.cancel !== "function") throw new Error("stream cancel");
              ignored.cancel();
              var branches = stream.tee();
              var reader = branches[0].getReader();
              reader.read().then(function (first) {
                return reader.read().then(function (second) {
                  return reader.read().then(function (end) {
                    document.documentElement.setAttribute(
                      "data-stream",
                      first.value + second.value + (end.done ? "done" : "open")
                    );
                  });
                });
              });
            </script></body></html>"#,
        );
        let session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        let html = session
            .document()
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (session.document().element_name(NodeId(index)) == Some("html"))
                    .then_some(NodeId(index))
            })
            .unwrap();
        assert_eq!(session.document().attr(html, "data-stream"), Some("abdone"));
    }

    #[test]
    fn an_inserted_script_runs_when_it_is_connected() {
        let doc = parse_html(
            r#"<html><head></head><body>
            <script>
              var inline = document.createElement("script");
              inline.textContent = "document.documentElement.setAttribute('data-inline','yes')";
              document.body.appendChild(inline);
              var external = document.createElement("script");
              external.src = "/extra.js";
              external.addEventListener("load", function () {
                document.documentElement.setAttribute("data-load", "yes");
              });
              document.head.appendChild(external);
            </script>
            </body></html>"#,
        );
        let session = Session::open_at(doc, "https://example.test/", |src| {
            src.ends_with("/extra.js").then(|| {
                "document.documentElement.setAttribute('data-src','yes')".to_string()
            })
        });
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        let html = session
            .document()
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (session.document().element_name(NodeId(index)) == Some("html"))
                    .then_some(NodeId(index))
            })
            .unwrap();
        assert_eq!(session.document().attr(html, "data-inline"), Some("yes"));
        assert_eq!(session.document().attr(html, "data-src"), Some("yes"));
        assert_eq!(session.document().attr(html, "data-load"), Some("yes"));
    }

    #[test]
    fn message_channel_and_post_message_are_macrotasks() {
        let doc = parse_html(
            r#"<html><body></body>
            <script>
              var seen = [];
              var channel = new MessageChannel();
              channel.port1.addEventListener("message", function (event) {
                seen.push("listener:" + event.data);
              });
              channel.port2.postMessage("early");
              channel.port1.start();
              channel.port2.postMessage("late");
              channel.port1.onmessage = function (event) { seen.push("handler:" + event.data); };
              channel.port2.postMessage("after");
              window.addEventListener("message", function (event) { seen.push("win:" + event.data); });
              window.onmessage = function (event) { seen.push("on:" + event.data); };
              window.postMessage("hello");
              queueMicrotask(function () { seen.push("micro"); });
              var before = performance.now();
              seen.push("script");
              setTimeout(function () {
                var after = performance.now();
                document.documentElement.setAttribute("data-order", seen.join(","));
                document.documentElement.setAttribute("data-now", String(after >= before));
              }, 0);
            </script></html>"#,
        );
        let mut session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        let _ = session.advance(0);
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
        let order = session
            .document()
            .attr(html, "data-order")
            .unwrap_or("")
            .to_string();
        assert_eq!(
            order,
            "script,micro,handler:early,listener:early,handler:late,listener:late,handler:after,listener:after,win:hello,on:hello"
        );
        assert_eq!(session.document().attr(html, "data-now"), Some("true"));
    }

    #[test]
    fn an_adopted_style_sheet_is_collected_for_layout() {
        let mut doc = parse_html(
            r#"<div id="host"></div><script>
            var sheet = new CSSStyleSheet();
            sheet.replaceSync("p{color:#111}");
            document.adoptedStyleSheets = [sheet];
            var root = document.getElementById("host").attachShadow({mode:"open"});
            var local = new CSSStyleSheet();
            local.replaceSync(":host{display:block}");
            root.adoptedStyleSheets = [local];
            </script>"#,
        );
        let log = run_scripts(&mut doc, |_| None);
        assert!(log.is_empty(), "{log:?}");
        let joined = doc.stylesheets.join("\n");
        assert!(joined.contains("color"), "{joined}");
        assert!(
            joined.contains("display:block") && !joined.contains(":host"),
            "{joined}"
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
    fn document_cookie_reads_script_cookies_and_hides_httponly() {
        let href = "https://cookie-case.invalid/page";
        let url = mithu_url::Url::parse(href).unwrap();
        mithu_net::store_set_cookie(&url, "secret=1; HttpOnly; Path=/");
        let doc = parse_html(
            r#"<script>
              document.cookie = "a=b; Path=/";
              var seen = document.cookie;
              document.title = seen.indexOf("a=b") >= 0 && seen.indexOf("secret") < 0 ? "ok" : seen;
              var event = document.createEvent("Event");
              event.initEvent("foo", true, true);
              event.preventDefault();
              if (!event.defaultPrevented || "abcd".substr(1, 2) !== "bc" || !document.documentElement.isConnected) {
                document.title = "api";
              }
              if (!document.location || document.location.href.indexOf("cookie-case") < 0) {
                document.title = "location";
              }
            </script>"#,
        );
        let session = Session::open_at(doc, href, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "ok");
    }

    #[test]
    fn a_mixin_class_is_constructed_with_new() {
        let doc = parse_html(
            r#"<body><script>
              function mixin(base) { return class extends base {}; }
              class El extends mixin(HTMLElement) {
                constructor() {
                  super();
                  this.setAttribute("ready", new.target === El ? "yes" : "no");
                }
              }
              customElements.define("x-mix", El);
              var el = document.createElement("x-mix");
              document.body.appendChild(el);
              document.title = el.getAttribute("ready") || "missing";
              if (!/\bclass\b/.test((class {}).toString())) document.title = "tostring";
              if ((function () { return 1; }).toString().indexOf("return 1") < 0) document.title = "fntext";
              if (HTMLElement.prototype.attachShadow.toString().indexOf("[native code]") < 0) document.title = "shadow";
            </script></body>"#,
        );
        let session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "yes");
    }

    #[test]
    fn a_text_mutation_reaches_the_observer() {
        let doc = parse_html(
            r#"<body><script>
              var node = document.createTextNode("a");
              document.body.appendChild(node);
              var observer = new MutationObserver(function (records) {
                document.title = records.length && records[0].type === "characterData" ? "ok" : "bad";
              });
              observer.observe(node, { characterData: true });
              node.data = "b";
            </script></body>"#,
        );
        let session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "ok");
    }

    #[test]
    fn a_child_list_record_lists_added_nodes() {
        let doc = parse_html(
            r#"<body><div id="host"></div><script>
              var host = document.getElementById("host");
              var observer = new MutationObserver(function (records) {
                var added = records[0] && records[0].addedNodes;
                document.title = added && added.length === 1 && added[0].id === "item" ? "ok" : "bad";
              });
              observer.observe(host, { childList: true });
              var item = document.createElement("span");
              item.id = "item";
              host.appendChild(item);
            </script></body>"#,
        );
        let session = Session::open(doc, |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "ok");
    }

    #[test]
    fn style_and_shadow_and_custom_elements_are_callable() {
        let doc = parse_html(
            r#"<body><script>
              if (document.readyState !== "loading") document.title = "ready:" + document.readyState;
              var probe = document.createElement("div");
              probe.style.cssText = "user-select: contain;";
              if (probe.style.getPropertyValue("user-select") !== "contain") {
                document.title = "style";
              }
              var shadow = probe.attachShadow({mode: "open"});
              shadow.innerHTML = "<span id='in'>x</span>";
              if (!shadow || shadow.mode !== "open" || !probe.shadowRoot) {
                document.title = "shadow";
              }
              class Box extends HTMLElement {}
              class Stamp extends HTMLElement {
                constructor() { super(); this.built = "yes"; }
                connectedCallback() { this.setAttribute("ready", this.built || "no"); }
              }
              customElements.define("x-stamp", Stamp);
              var stamp = document.createElement("x-stamp");
              document.body.appendChild(stamp);
              if (stamp.getAttribute("ready") !== "yes") document.title = "stamp:" + stamp.getAttribute("ready");
              class Mid extends HTMLElement { constructor() { super(); this.mid = "a"; } }
              class Top extends Mid {
                constructor() { super(); this.top = "b"; }
                connectedCallback() { this.setAttribute("ready", (this.mid || "") + (this.top || "")); }
              }
              customElements.define("x-top", Top);
              var topEl = document.createElement("x-top");
              document.body.appendChild(topEl);
              if (topEl.getAttribute("ready") !== "ab") document.title = "chain:" + topEl.getAttribute("ready");
              customElements.define("x-box", Box);
              var box = document.createElement("x-box");
              if (!(box instanceof Box) || customElements.get("x-box") !== Box) {
                document.title = "custom";
              }
              if (typeof window.__shady_native_addEventListener !== "function") {
                document.title = "shady";
              }
              if (typeof Element.prototype.getAttribute !== "function" || typeof Node.prototype.replaceChild !== "function") {
                document.title = "proto";
              }
              var inert = document.implementation.createHTMLDocument("inert");
              if (!inert.documentElement || !inert.body) document.title = "inert";
              if (document.domain !== "dom-case.invalid") document.title = "domain";
              if (typeof performance.timing.navigationStart !== "number" || typeof performance.timing.responseStart !== "number") {
                document.title = "timing";
              }
              var canvas = document.createElement("canvas");
              var ctx = canvas.getContext("2d");
              if (!ctx || typeof ctx.fillRect !== "function" || canvas.getContext("webgl") !== null || ctx.measureText("ab").width < 1) {
                document.title = "canvas";
              }
              var ev = document.createEvent("CustomEvent");
              ev.initCustomEvent("spfready", true, true, {ok: 1});
              if (ev.type !== "spfready" || !ev.detail || ev.detail.ok !== 1) document.title = "event";
              var tpl = document.createElement("template");
              tpl.innerHTML = "<b id='moved'>z</b>";
              tpl.content.insertBefore(document.createTextNode("a"), tpl.content.firstChild);
              var copy = tpl.content.cloneNode(true);
              document.body.appendChild(copy);
              if (!document.getElementById("moved")) document.title = "moved";
              var range = new Range();
              range.selectNodeContents(document.body);
              if (!range.getBoundingClientRect() || range.getClientRects().length !== 1) document.title = "range";
              if (!document.scripts || typeof document.scripts.length !== "number" || document.scripts.length < 1) document.title = "scripts";
              if (getComputedStyle(document.documentElement).fontSize.replace("px","") !== "16") document.title = "font";
              if (navigator.mimeTypes.length !== 0 || navigator.plugins.length !== 0) document.title = "plugins";
              if (!screen.width || !navigator.userAgentData || navigator.userAgentData.mobile !== false) document.title = "screen";
              if (navigator.userAgent.indexOf("Mithu/0.1.0") < 0 || navigator.userAgent.indexOf("Chrome/157.0.0.0") < 0) document.title = "ua";
              if (!navigator.userAgentData.brands || navigator.userAgentData.brands.length !== 3) document.title = "brands";
              var grouped = Object.groupBy([0,1,2,3], function (n) { return n % 2 ? "odd" : "even"; });
              if (!grouped.even || grouped.even.length !== 2 || [].group(function (n) { return n; }).length) document.title = "group";
              var described = Object.getOwnPropertyDescriptor({ email() { return 1; } }, "email");
              if (!described || typeof described.value !== "function") document.title = "desc";
              var inherited = [];
              for (var key in {}) inherited.push(key);
              var methods = { email() { return 1; }, url() { return 2; } };
              var missing = [];
              for (var key2 in methods) {
                if (!Object.getOwnPropertyDescriptor(methods, key2)) missing.push(key2);
              }
              var hostEl = document.createElement("div");
              var hostMissing = [];
              for (var hostKey in hostEl) {
                if (!Object.getOwnPropertyDescriptor(hostEl, hostKey)) hostMissing.push(hostKey);
              }
              if (hostMissing.length) document.title = "host:" + hostMissing.join(",");
              customElements.whenDefined("x-stamp").then(function (ctor) {
                if (document.title === "when") document.title = ctor === Stamp ? "ok" : "when-bad";
              });
              if (window.top !== window || window.parent !== window) document.title = "top";
              if (inherited.length || missing.length || typeof WheelEvent !== "function") {
                document.title = "forin:" + inherited.join(",") + "/" + missing.join(",") + ":" + typeof WheelEvent;
              }
              if (document.title === "") document.title = "when";
            </script></body>"#,
        );
        let session = Session::open_at(doc, "https://dom-case.invalid/", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "ok");
    }

    fn js_fixtures(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "js") {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn test262_subset_reports_a_pass_rate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/test262");
        let files = js_fixtures(&root);
        assert!(files.len() >= 20, "vendored test262 files: {}", files.len());
        let harness = r#"function assert(value, message) { if (value !== true) throw new Error(message || 'assert'); }
            assert.sameValue = function (actual, expected, message) {
              if (actual !== expected) throw new Error(message || String(actual) + ' !== ' + String(expected));
            };"#;
        let mut passed = 0usize;
        let mut failed = Vec::new();
        for path in &files {
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("case");
            let body = std::fs::read_to_string(path).unwrap_or_default();
            let doc = parse_html(&format!(
                "<script>{harness}{body}; document.title = 'ok';</script>"
            ));
            let session = Session::open(doc, |_| None);
            if session.document().title == "ok" && session.logs().is_empty() {
                passed += 1;
            } else {
                failed.push(name.to_string());
            }
        }
        let total = files.len();
        let rate = passed as f64 / total as f64;
        let report = format!(
            "test262 subset {passed}/{total} ({:.0}%) failed: {}",
            rate * 100.0,
            failed.join(",")
        );
        eprintln!("{report}");
        assert!(
            rate >= 0.7,
            "test262 subset pass rate {passed}/{total} ({:.0}%) failed {failed:?}",
            rate * 100.0
        );
    }

    #[test]
    fn wpt_dom_subset_reports_a_pass_rate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/dom");
        let files = js_fixtures(&root);
        assert!(!files.is_empty());
        let mut script = String::from("var passed = 0; var failed = '';");
        for path in &files {
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("case");
            let body = std::fs::read_to_string(path).unwrap();
            script.push_str(&format!(
                "try {{ {body}; passed += 1; }} catch (error) {{ failed += '{name}:' + error + ','; }}"
            ));
        }
        script.push_str(&format!(
            "document.title = passed + '/' + {} + ' ' + failed;",
            files.len()
        ));
        let doc = parse_html(&format!("<body><script>{script}</script></body>"));
        let session = Session::open(doc, |_| None);
        let title = session.document().title.clone();
        let (passed, rest) = title.split_once('/').unwrap_or(("0", "0"));
        let total = rest.split_whitespace().next().unwrap_or("0");
        eprintln!("wpt dom subset {passed}/{total} {title}");
        assert_eq!(total, files.len().to_string(), "{title}");
        assert_eq!(passed, total, "{title}");
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
    fn snapshots_stay_stable_and_comments_can_be_created() {
        let doc = parse_html(
            r#"<body></body><script>
              var state = { __NA: true };
              history.replaceState(state, "", location.href);
              if (history.state !== state) throw new Error("state");
              var list = matchMedia("(min-width: 800px)");
              if (matchMedia("(min-width: 800px)") !== list || list.matches !== true) {
                throw new Error("mql");
              }
              if (innerWidth !== 1280) throw new Error("width");
              var mark = document.createComment("$?");
              if (mark.nodeType !== 8 || mark.data !== "$?") throw new Error("comment");
              var fragment = document.createDocumentFragment();
              fragment.appendChild(mark);
              if (fragment.nodeType !== 11 || fragment.firstChild !== mark) throw new Error("fragment");
              var box = document.createElement("div");
              document.body.appendChild(box);
              var seen = 0;
              new ResizeObserver(function () { seen += 1; }).observe(box);
              new IntersectionObserver(function () { seen += 1; }).observe(box);
              if (seen !== 0) throw new Error("sync observer " + seen);
              var rect = box.getBoundingClientRect();
              if (box.getBoundingClientRect() !== rect) throw new Error("rect");
              if (document.elementFromPoint(10, 10) !== null) throw new Error("hit");
              if (document.elementsFromPoint(10, 10).length !== 0) throw new Error("hits");
              if (typeof PerformanceObserver !== "function") throw new Error("po");
              if (PerformanceObserver.supportedEntryTypes.indexOf("layout-shift") < 0) throw new Error("shift");
              var imported = document.importNode(mark, true);
              if (imported.nodeType !== 8 || imported.data !== "$?") throw new Error("import");
              var walker = document.createTreeWalker(fragment, NodeFilter.SHOW_COMMENT);
              var stepped = walker.nextNode();
              if (!stepped || stepped.data !== "$?") throw new Error("walker");
              var named = document.createElement("div");
              named.setAttribute("id", "z");
              if ([...named.attributes][0].name !== "id") throw new Error("attrs");
              if (!document.firstElementChild) throw new Error("first");
              document.title = "stable";
            </script>"#,
        );
        let mut session = Session::open_at(doc, "https://example.com/docs", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "stable");
        assert!(session.take_navigation().is_none());
        session.advance(0);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
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
              channel.port1.onmessage = function (event) {
                document.documentElement.setAttribute("data-port", event.data);
              };
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
                "",
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
        let _ = session.advance(0);
        let port = session
            .document()
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (session.document().element_name(mithu_dom::NodeId(index)) == Some("html"))
                    .then_some(mithu_dom::NodeId(index))
            })
            .and_then(|html| session.document().attr(html, "data-port").map(str::to_string));
        assert_eq!(port.as_deref(), Some("ported"));
        let title = session.document().title.clone();
        assert_eq!(
            title,
            "https://example.com/pixel.png||36|cdn.example|?q=mithu||dark|sd|function|function|function|true|Linux x86_64|function|ping|true|1"
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

    #[test]
    fn abort_template_and_fonts() {
        let doc = parse_html(
            r#"<template id="t"><p id="inner">Hi</p></template>
            <script>
              var c = new AbortController();
              var seen = "";
              c.signal.addEventListener("abort", function () { seen = "aborted"; });
              c.abort("stop");
              var live = new AbortController();
              live.signal.throwIfAborted();
              var template = document.getElementById("t");
              var copy = template.content.cloneNode(true);
              document.body.appendChild(copy);
              var face = new FontFace("Test", "url(x)").load();
              fetch("/nope", { signal: AbortSignal.abort("stop") }).then(function () {
                template.setAttribute("data-fetch", "ok");
              }, function () {
                template.setAttribute("data-fetch", "aborted");
              });
              document.title = [
                c.signal.aborted,
                c.signal.reason,
                seen,
                template instanceof HTMLTemplateElement,
                template instanceof HTMLElement,
                template.content.nodeType,
                template.content.childNodes.length,
                template.content.querySelector("p").id,
                document.fonts.status,
                typeof document.fonts.ready.then,
                typeof face.then,
                document.createElement("button") instanceof HTMLButtonElement,
                document.createElement("button") instanceof HTMLElement,
                document.createElement("button") instanceof Element,
                typeof console.info
              ].join("|");
            </script>"#,
        );
        let session = Session::open_at(doc, "https://example.com/", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(
            session.document().title,
            "true|stop|aborted|true|true|11|1|inner|loading|function|function|true|true|true|function"
        );
        let template = session.document().get_element_by_id("t").unwrap();
        assert_eq!(
            session.document().attr(template, "data-fetch"),
            Some("aborted")
        );
        assert!(session.document().get_element_by_id("inner").is_some());
        let template = session.document().get_element_by_id("t").unwrap();
        assert!(
            session.document().node(template).children.is_empty(),
            "template contents stay in the fragment"
        );
    }

    #[test]
    fn hydration_dom_matches_server_markup() {
        let doc = parse_html(
            r#"<div id="root">Hello <span id="name">world</span></div>
            <p id="note"><!--$-->Hi<!--/$--></p>
            <template id="view"><b id="secret">x</b></template>
            <img id="pic" alt="a &amp; b" hidden>
            <script>
              var root = document.getElementById("root");
              var text = root.childNodes[0];
              var note = document.getElementById("note");
              var mark = note.childNodes[0];
              var view = document.getElementById("view");
              var pic = document.getElementById("pic");
              if (text.nodeType !== 3 || text.nodeValue !== "Hello ") throw new Error("text:" + text.nodeValue);
              if (root.childNodes[1].id !== "name") throw new Error("span");
              if (note.textContent !== "Hi") throw new Error("textContent:" + note.textContent);
              if (!mark || mark.nodeType !== 8 || mark.nodeValue !== "$") throw new Error("comment:" + (mark && mark.nodeValue));
              if (note.nodeValue !== null) throw new Error("element nodeValue");
              if (view.childNodes.length !== 0) throw new Error("template children " + view.childNodes.length);
              if (view.content.childNodes.length !== 1) throw new Error("fragment");
              if (document.getElementById("secret")) throw new Error("template id leaked");
              if (view.content.querySelector("b").id !== "secret") throw new Error("fragment query");
              if (pic.getAttribute("alt") !== "a & b" || pic.getAttribute("hidden") !== "") throw new Error("attrs:" + pic.getAttribute("alt"));
              if (pic.innerHTML !== "") throw new Error("img html");
              var meta = document.createElement("meta");
              meta.setAttribute("name", "viewport");
              meta.setAttribute("content", "width=device-width");
              var copy = document.createElement("meta");
              copy.setAttribute("content", "width=device-width");
              copy.setAttribute("name", "viewport");
              if (typeof meta.isEqualNode !== "function" || !meta.isEqualNode(copy)) throw new Error("isEqualNode");
              copy.setAttribute("content", "other");
              if (meta.isEqualNode(copy)) throw new Error("isEqualNode differed");
              var cloned = meta.cloneNode(true);
              cloned.setAttribute("data-next-head", "");
              if (meta.isEqualNode(cloned) || !meta.isSameNode(meta) || meta.isSameNode(cloned)) throw new Error("clone");
              var nonce = document.createElement("meta");
              nonce.setAttribute("nonce", "abc");
              if (nonce.nonce !== "abc") throw new Error("nonce");
              var hello = document.createTextNode("Hi");
              if (!hello.isEqualNode(document.createTextNode("Hi"))) throw new Error("text equal");
              document.title = "hydrated";
            </script>"#,
        );
        let session = Session::open_at(doc, "https://react.dev/", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "hydrated");
    }

    #[test]
    fn the_common_window_surface_is_constructable() {
        let doc = parse_html(
            r#"<dialog id="ask"></dialog><span id="icon"></span>
            <script>
              var dialog = document.getElementById("ask");
              if (!(dialog instanceof HTMLDialogElement)) throw new Error("dialog type");
              if (!(document.createElement("br") instanceof HTMLBRElement)) throw new Error("br");
              if (!(document.createElement("audio") instanceof HTMLMediaElement)) throw new Error("media");
              dialog.showModal();
              if (!dialog.open) throw new Error("open");
              dialog.close("yes");
              if (dialog.open || dialog.returnValue !== "yes") throw new Error("close:" + dialog.returnValue);
              var slot = document.createElement("slot");
              slot.appendChild(document.createElement("b"));
              if (slot.assignedElements().length !== 1) throw new Error("slot");
              if (CSS.supports("display", "grid") !== true) throw new Error("supports");
              if (CSS.escape("a b") !== "a\\ b") throw new Error("escape:" + CSS.escape("a b"));
              if (String(CSS.px(12)) !== "12px" || String(CSS.number(2)) !== "2") throw new Error("css unit");
              if (typeof navigation.navigate !== "function") throw new Error("navigation");
              if (typeof navigation.currentEntry.getState !== "function") throw new Error("entry");
              var any = AbortSignal.any([AbortSignal.abort("stop")]);
              if (!any.aborted) throw new Error("any");
              if (typeof AbortSignal.timeout(1000).aborted !== "boolean") throw new Error("timeout");
              var cloned = structuredClone({ n: 1, list: [2] });
              if (cloned.n !== 1 || cloned.list[0] !== 2) throw new Error("clone");
              var headers = new Headers("Accept: text/html\nX-QT-AUTH: token");
              if (headers.get("x-qt-auth") !== "token") throw new Error("headers");
              var response = new Response("{\"ok\":1}", { status: 201, headers: headers, url: "https://example.com/a" });
              if (response.status !== 201 || !response.ok || response.headers.get("accept") !== "text/html") throw new Error("response");
              if (Response.json({ a: 1 }).headers.get("content-type") !== "application/json") throw new Error("json");
              var request = new Request("/path", { method: "post", body: "hi", headers: { "content-type": "text/plain" } });
              if (request.method !== "POST" || request.headers.get("content-type") !== "text/plain") throw new Error("request");
              if (!request.url.endsWith("/path")) throw new Error("request url " + request.url);
              var blob = new Blob(["ab"]);
              if (blob.size !== 2) throw new Error("blob");
              if (!(new File(["a"], "a.txt") instanceof Blob)) throw new Error("file");
              var form = new FormData();
              form.append("q", "1");
              if (form.get("q") !== "1") throw new Error("form");
              if (!URL.canParse("https://example.com") || URL.canParse("http://[")) throw new Error("canParse");
              if ([1, 2, 3].findLast(function (n) { return n < 3; }) !== 2) throw new Error("findLast");
              if ([3, 1, 2].toSorted().join() !== "1,2,3") throw new Error("toSorted");
              var grouped = Object.groupBy(["aa", "b"], function (item) { return item.length; });
              if (grouped[2].length !== 1) throw new Error("groupBy");
              var pair = Promise.withResolvers();
              if (typeof pair.resolve !== "function") throw new Error("withResolvers");
              if (new WeakRef({ id: 7 }).deref().id !== 7) throw new Error("weak");
              new FinalizationRegistry(function () {});
              if (typeof location.assign !== "function" || location.toString().indexOf("example.com") < 0) throw new Error("location");
              var named = document.createElement("input");
              named.setAttribute("name", "q");
              document.documentElement.appendChild(named);
              if (document.getElementsByName("q").length !== 1) throw new Error("by name");
              document.title = "platform";
            </script>"#,
        );
        let session = Session::open_at(doc, "https://example.com/docs", |_| None);
        assert!(session.logs().is_empty(), "{:?}", session.logs());
        assert_eq!(session.document().title, "platform");
        let dialog = session.document().get_element_by_id("ask").unwrap();
        assert!(session.document().attr(dialog, "open").is_none());
    }
}
