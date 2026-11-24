//! Browser-process engine thread.
//!
//! The winit thread paints and receives input. Servo is created and pumped
//! here, so a long layout or script turn does not run on the thread that
//! owns the window. The window thread only queues a job and returns.
//! Servo wakes this thread through [`EngineWake`].

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mithu_shell::{Browser, EngineWake, bind_engine_wake};
use winit::event_loop::EventLoopProxy;

pub enum UiAction {
    Exit,
    Minimize,
    Maximize,
    Drag,
}

/// The last finished frame. The window thread takes an `Arc` to it instead
/// of copying 4 bytes per pixel under the lock, the same hand-off Chromium's
/// viz uses between the compositor and the display.
pub struct Shared {
    pub pixels: Arc<Vec<u32>>,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub seq: u64,
}

pub struct Session {
    pub browser: Browser,
    pub modifiers_control: bool,
    pub modifiers_alt: bool,
    pub modifiers_shift: bool,
    pub cursor: (f32, f32),
    pub width: f32,
    pub height: f32,
    shared: Arc<Mutex<Shared>>,
    proxy: EventLoopProxy<()>,
    painted: u64,
}

impl Session {
    pub fn publish(&mut self) {
        let revision = self.browser.content_revision();
        if revision == self.painted && self.shared.lock().expect("frame").seq > 0 {
            return;
        }
        let width = self.width.max(1.0);
        let height = self.height.max(1.0);
        let pixels = self.browser.render(width, height);
        let title = self.browser.title();
        let mut shared = self.shared.lock().expect("frame");
        shared.pixels = Arc::new(pixels);
        shared.width = width.round() as u32;
        shared.height = height.round() as u32;
        shared.title = title;
        shared.seq += 1;
        self.painted = self.browser.content_revision();
        drop(shared);
        let _ = self.proxy.send_event(());
    }
}

pub struct Frame {
    pub pixels: Arc<Vec<u32>>,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub seq: u64,
}

type Job = Box<dyn FnOnce(&mut Session) + Send>;

pub struct Engine {
    tx: Sender<Job>,
    wake: Arc<EngineWake>,
    shared: Arc<Mutex<Shared>>,
    actions: Arc<Mutex<Vec<UiAction>>>,
}

impl Engine {
    pub fn spawn(proxy: EventLoopProxy<()>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let wake = EngineWake::new();
        let shared = Arc::new(Mutex::new(Shared {
            pixels: Arc::new(Vec::new()),
            width: 0,
            height: 0,
            title: String::new(),
            seq: 0,
        }));
        let wake_thread = wake.clone();
        let shared_thread = shared.clone();
        std::thread::Builder::new()
            .name("mithu-engine".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || engine_loop(rx, wake_thread, shared_thread, proxy))
            .expect("start engine");
        Self {
            tx,
            wake,
            shared,
            actions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Queue work for the engine thread and return. The window thread does not
    /// wait for layout, script, or paint. Window actions land in [`Self::take_actions`].
    pub fn post(&self, job: impl FnOnce(&mut Session) -> Vec<UiAction> + Send + 'static) {
        let slot = self.actions.clone();
        let wrapped: Job = Box::new(move |session| {
            let produced = job(session);
            if !produced.is_empty() {
                slot.lock().expect("actions").extend(produced);
                let _ = session.proxy.send_event(());
            }
        });
        if self.tx.send(wrapped).is_err() {
            mithu_host::report_fatal("engine stopped");
        }
        self.wake.poke();
    }

    pub fn take_actions(&self) -> Vec<UiAction> {
        std::mem::take(&mut *self.actions.lock().expect("actions"))
    }

    pub fn frame(&self) -> Frame {
        let shared = self.shared.lock().expect("frame");
        Frame {
            pixels: shared.pixels.clone(),
            width: shared.width,
            height: shared.height,
            title: shared.title.clone(),
            seq: shared.seq,
        }
    }

    pub fn frame_seq(&self) -> u64 {
        self.shared.lock().expect("frame").seq
    }
}

fn engine_loop(
    rx: Receiver<Job>,
    wake: Arc<EngineWake>,
    shared: Arc<Mutex<Shared>>,
    proxy: EventLoopProxy<()>,
) {
    bind_engine_wake(wake.clone());
    let mut session = Session {
        browser: Browser::new(),
        modifiers_control: false,
        modifiers_alt: false,
        modifiers_shift: false,
        cursor: (0.0, 0.0),
        width: 1200.0,
        height: 800.0,
        shared,
        proxy,
        painted: 0,
    };
    session.browser.set_cooperative_scripts(true);
    session.browser.follow_os_theme();
    session.browser.resize(session.width, session.height);
    session.publish();
    loop {
        // Servo pokes `wake` through its EventLoopWaker whenever the
        // constellation, a content process, or the refresh driver has work,
        // so the timeout is only a safety net. Chromium's browser UI thread
        // likewise sleeps in its message pump until a task is posted instead
        // of polling. (Before: an 8 ms poll during loads and animations.)
        let idle = session.browser.load_settled() && !session.browser.animation_frames_pending();
        let mut limit = if idle {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(50)
        };
        if let Some(due) = session.browser.scroll_frame_due() {
            limit = limit.min(due);
        }
        if let Some(due) = session.browser.paint_due() {
            limit = limit.min(due);
        }
        wake.wait(limit);
        wake.take();
        while let Ok(job) = rx.try_recv() {
            job(&mut session);
        }
        session.browser.tick_scroll();
        session.browser.spin_once();
        session.publish();
    }
}
