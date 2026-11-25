#!/usr/bin/env python3
"""Share one in-process event loop for pages with no site, and cap site processes.

Servo starts a new content process whenever a document has no registrable
domain, so every about:blank tab becomes its own process. This patch keeps
those documents, and mithu: pages, on one in-process event loop. http(s)
sites still get a content process, and past four site processes a new site
reuses one that already exists.

That in-process thread is also pinned until shutdown. Closing the last blank
or internal page used to drop the thread, which drops its cloned namespace
sender. On Windows, ipc-channel then reports the constellation's routed
receiver disconnected even though the constellation still holds a sender.
The arm that panics is the namespace receiver, but Servo copy-pasted the
expect text "Unexpected script channel panic in constellation" (this is
constellation.rs:1244 after the line shift from the edits above). A closed
script or namespace receiver is logged, only event loops that no longer
answer are marked crashed, and the other tabs stay up. A closed hang-monitor
alert channel is logged and ignored. The crash page says
"This page crashed" and offers Reload.

The Servo submodule itself is not forked.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONSTELLATION = ROOT / "third_party/servo/components/constellation/constellation.rs"
EVENT_LOOP = ROOT / "third_party/servo/components/constellation/event_loop.rs"
SERVO = ROOT / "third_party/servo/components/servo/servo.rs"
MARKER = "MITHU_PROCESS_CAP"

OLD_BLANK = """            // This might happen if a new Pipeline is requested and in the meantime the parent
            // Pipeline has shut down. In this case, just make a new ScriptThread.
            return None;
        }"""

NEW_BLANK = """            // No creator. Fall through so pages with no site share one event loop
            // instead of starting a content process per blank tab. MITHU_PROCESS_CAP
        }"""

OLD_CREATE = """        let registered_domain_name = if load_data
            .creation_sandboxing_flag_set
            .contains(SandboxingFlagSet::SANDBOXED_ORIGIN_BROWSING_CONTEXT_FLAG)
        {
            None
        } else {
            registered_domain_name(&load_data.url)
        };

        if let Some(event_loop) = self.get_event_loop_for_new_pipeline(
            load_data,
            webview_id,
            parent_pipeline_id,
            &registered_domain_name,
        ) {
            return Ok(event_loop);
        }

        let event_loop = EventLoop::spawn(self, is_private)?;
        if let Some(registered_domain_name) = registered_domain_name {
            self.set_event_loop(&event_loop, registered_domain_name, &webview_id);
        }
        Ok(event_loop)
    }"""

NEW_CREATE = """        let sandboxed = load_data
            .creation_sandboxing_flag_set
            .contains(SandboxingFlagSet::SANDBOXED_ORIGIN_BROWSING_CONTEXT_FLAG);
        let mut registered_domain_name = if sandboxed {
            None
        } else {
            registered_domain_name(&load_data.url)
        };
        // Pages with no registrable domain (about:, mithu:, data:) share one
        // in-process event loop. Site processes stop at four and then reuse.
        // MITHU_PROCESS_CAP
        let internal = registered_domain_name.is_none() && !sandboxed;
        if internal {
            registered_domain_name = Some(Host::Domain("mithu-internal.invalid".to_owned()));
        }

        if let Some(event_loop) = self.get_event_loop_for_new_pipeline(
            load_data,
            webview_id,
            parent_pipeline_id,
            &registered_domain_name,
        ) {
            return Ok(event_loop);
        }

        if !internal &&
            let Some(host) = registered_domain_name.clone() &&
            let Some(group) = self.browsing_context_group_for_webview_id(&webview_id)
        {
            let group = group.borrow();
            let mut site_loops = Vec::new();
            for (key, weak) in &group.event_loops {
                if matches!(key, Host::Domain(name) if name == "mithu-internal.invalid") {
                    continue;
                }
                if let Some(event_loop) = weak.upgrade() {
                    site_loops.push(event_loop);
                }
            }
            if site_loops.len() >= 4 {
                drop(group);
                let event_loop = site_loops.remove(0);
                self.set_event_loop(&event_loop, host, &webview_id);
                return Ok(event_loop);
            }
        }

        let event_loop = EventLoop::spawn(self, is_private, internal)?;
        if internal {
            // Closing the last about:/mithu:/data: page must not drop this
            // thread. Its cloned namespace sender is what Windows ipc-channel
            // treats as the last client, and the constellation then panics in
            // the namespace arm (the message text says "script channel").
            // MITHU_PROCESS_CAP
            if self.pinned_internal_event_loop.is_none() {
                self.pinned_internal_event_loop = Some(event_loop.clone());
            }
        }
        if let Some(registered_domain_name) = registered_domain_name {
            self.set_event_loop(&event_loop, registered_domain_name, &webview_id);
        }
        Ok(event_loop)
    }"""

OLD_SPAWN = """        #[cfg(feature = "multiprocess")]
        let event_loop = if opts::get().multiprocess {
            Self::spawn_in_process(constellation, initial_script_state)?
        } else {
            Self::spawn_in_thread(constellation, initial_script_state)
        };
        #[cfg(not(feature = "multiprocess"))]
        let event_loop = Self::spawn_in_thread(constellation, initial_script_state);"""

NEW_SPAWN = """        #[cfg(feature = "multiprocess")]
        let event_loop = if opts::get().multiprocess && !in_process {
            Self::spawn_in_process(constellation, initial_script_state)?
        } else {
            Self::spawn_in_thread(constellation, initial_script_state)
        };
        #[cfg(not(feature = "multiprocess"))]
        let event_loop = {
            let _ = in_process;
            Self::spawn_in_thread(constellation, initial_script_state)
        };"""

OLD_SIG = """    pub(crate) fn spawn<STF: ScriptThreadFactory, SWF: ServiceWorkerManagerFactory>(
        constellation: &mut Constellation<STF, SWF>,
        is_private: bool,
    ) -> Result<Rc<Self>, SendError> {"""

NEW_SIG = """    pub(crate) fn spawn<STF: ScriptThreadFactory, SWF: ServiceWorkerManagerFactory>(
        constellation: &mut Constellation<STF, SWF>,
        is_private: bool,
        in_process: bool,
    ) -> Result<Rc<Self>, SendError> {"""


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    replace_any(path, [old], new, label)


def replace_any(path: Path, olds: list[str], new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    if new in text:
        return
    for old in olds:
        if old in text:
            path.write_text(text.replace(old, new, 1), encoding="utf-8")
            return
    sys.exit(f"{path.name}: could not find {label}")


OLD_MONITOR = """                let (
                    background_monitor_register,
                    background_monitor_register_join_handle,
                    background_monitor_control_sender
                ) = if opts::get().multiprocess {
                    (None, None, None)
                } else {
                    let (
                        background_hang_monitor_control_ipc_sender,
                        background_hang_monitor_control_ipc_receiver,
                    ) = generic_channel::channel().expect("ipc channel failure");
                    let (register, join_handle) = HangMonitorRegister::init(
                        background_hang_monitor_ipc_sender.clone(),
                        background_hang_monitor_control_ipc_receiver,
                        opts::get().background_hang_monitor,
                    );
                    (
                        Some(register),
                        Some(join_handle),
                        Some(background_hang_monitor_control_ipc_sender),
                    )
                };"""

NEW_MONITOR = """                // The browser process keeps a hang monitor so pages with no site
                // can run on an in-process event loop while sites stay out of
                // process. Content processes still start their own monitor.
                // MITHU_PROCESS_CAP
                let (
                    background_monitor_register,
                    background_monitor_register_join_handle,
                    background_monitor_control_sender
                ) = {
                    let (
                        background_hang_monitor_control_ipc_sender,
                        background_hang_monitor_control_ipc_receiver,
                    ) = generic_channel::channel().expect("ipc channel failure");
                    let (register, join_handle) = HangMonitorRegister::init(
                        background_hang_monitor_ipc_sender.clone(),
                        background_hang_monitor_control_ipc_receiver,
                        opts::get().background_hang_monitor,
                    );
                    (
                        Some(register),
                        Some(join_handle),
                        Some(background_hang_monitor_control_ipc_sender),
                    )
                };"""


OLD_JS = """        let js_engine_setup = if !opts.multiprocess {
            Some(script::init())
        } else {
            None
        };"""

NEW_JS = """        // Opaque pages run a script thread in this process, so the JS engine
        // has to exist even when sites are out of process.
        // MITHU_PROCESS_CAP
        let js_engine_setup = Some(script::init());"""

OLD_MEDIA = """        if !opts.multiprocess {
            media_platform::init();
        }"""

NEW_MEDIA = """        // In-process event loops share this process with the browser.
        // MITHU_PROCESS_CAP
        media_platform::init();"""

OLD_NAMESPACE_RECV = """                1 => oper
                    .recv(&self.namespace_receiver)
                    .expect("Unexpected script channel panic in constellation")
                    .map(Request::PipelineNamespace),"""

NEW_NAMESPACE_RECV = """                1 => match oper.recv(&self.namespace_receiver) {
                    Ok(message) => message.map(Request::PipelineNamespace),
                    Err(error) => {
                        // This arm is the namespace receiver. Servo's expect text
                        // says "script channel" on this arm and on the script arm.
                        // Windows ipc-channel delivers RecvError here when an
                        // in-process script thread drops its cloned sender.
                        // MITHU_PROCESS_CAP
                        self.note_namespace_channel_disconnected(&error);
                        return;
                    },
                },"""

OLD_SCRIPT_RECV = """                2 => oper
                    .recv(&self.script_receiver)
                    .expect("Unexpected script channel panic in constellation")
                    .map(Request::Script),"""

# Earlier Mithu builds replaced only the script arm, and answered a disconnect
# by shutting down every pipeline. A browsing disconnect must not do that.
OLD_SCRIPT_RECV_SHUTDOWN = """                2 => match oper.recv(&self.script_receiver) {
                    Ok(message) => message.map(Request::Script),
                    Err(error) => {
                        // Windows ipc-channel reports the channel disconnected when
                        // an in-process script thread drops its sender on the way
                        // out, even though this thread still holds one. Finish
                        // shutdown instead of panicking. MITHU_PROCESS_CAP
                        if !self.shutting_down {
                            warn!("script channel disconnected ({error:?}); shutting down");
                            self.shutting_down = true;
                        }
                        self.pipelines.clear();
                        return;
                    },
                },"""

OLD_BHM_RECV = """                3 => oper
                    .recv(&self.background_hang_monitor_receiver)
                    .expect("Unexpected BHM channel panic in constellation")
                    .map(Request::BackgroundHangMonitor),"""

NEW_BHM_RECV = """                3 => match oper.recv(&self.background_hang_monitor_receiver) {
                    Ok(message) => message.map(Request::BackgroundHangMonitor),
                    Err(error) => {
                        // The hang monitor is not a page. A closed alert
                        // channel used to panic this thread and take the
                        // process with it. Retire the receiver and keep going.
                        // MITHU_PROCESS_CAP
                        warn!(
                            "background hang monitor disconnected ({error:?}); continuing. MITHU_PROCESS_CAP"
                        );
                        self.background_hang_monitor_receiver = crossbeam_channel::never();
                        return;
                    },
                },"""

NEW_SCRIPT_RECV = """                2 => match oper.recv(&self.script_receiver) {
                    Ok(message) => message.map(Request::Script),
                    Err(error) => {
                        // Same Windows disconnect as the namespace arm above.
                        // Do not panic and do not shut the browser down.
                        // MITHU_PROCESS_CAP
                        self.note_script_channel_disconnected(&error);
                        return;
                    },
                },"""

OLD_FIELD = """    /// A vector of [`JoinHandle`]s used to ensure full termination of threaded [`EventLoop`]s
    /// which are runnning in the same process.
    event_loop_join_handles: Vec<JoinHandle<()>>,"""

NEW_FIELD = """    /// A vector of [`JoinHandle`]s used to ensure full termination of threaded [`EventLoop`]s
    /// which are runnning in the same process.
    event_loop_join_handles: Vec<JoinHandle<()>>,

    /// Keeps the shared in-process script thread alive across tab closes.
    /// Dropped only from shutdown. MITHU_PROCESS_CAP
    pinned_internal_event_loop: Option<Rc<EventLoop>>,"""

OLD_FIELD_INIT = """                    event_loop_join_handles: Default::default(),"""

NEW_FIELD_INIT = """                    event_loop_join_handles: Default::default(),
                    pinned_internal_event_loop: None,"""

OLD_SHUTDOWN = """        debug!("Handling shutdown.");

        for join_handle in self.event_loop_join_handles.drain(..) {"""

NEW_SHUTDOWN = """        debug!("Handling shutdown.");

        // The pinned in-process thread exits only now, after pipelines are
        // gone, so its sender drop cannot race a live tab. MITHU_PROCESS_CAP
        self.pinned_internal_event_loop = None;

        for join_handle in self.event_loop_join_handles.drain(..) {"""

OLD_HANDLER_ANCHOR = """    fn handle_request_for_pipeline_namespace(&mut self, request: PipelineNamespaceRequest) {"""

NEW_HANDLER_ANCHOR = """    /// The namespace or script router closed. Retire that receiver so the
    /// select loop does not spin, and crash only the event loops that no
    /// longer accept messages. MITHU_PROCESS_CAP
    fn note_namespace_channel_disconnected(&mut self, error: &impl core::fmt::Debug) {
        self.namespace_receiver = crossbeam_channel::never();
        self.handle_routed_receiver_disconnected("namespace channel", error);
    }

    fn note_script_channel_disconnected(&mut self, error: &impl core::fmt::Debug) {
        self.script_receiver = crossbeam_channel::never();
        self.handle_routed_receiver_disconnected("script channel", error);
    }

    fn handle_routed_receiver_disconnected(&mut self, which: &str, error: &impl core::fmt::Debug) {
        warn!("{which} disconnected ({error:?}); other tabs stay open. MITHU_PROCESS_CAP");
        if self.shutting_down {
            self.pipelines.clear();
            return;
        }
        // One small message. A live script thread accepts it. A thread that
        // already dropped its receiver returns immediately, so this does not
        // wait on a full pipe while that thread is blocked on us.
        let event_loops = self.event_loops();
        let mut dead = Vec::new();
        for event_loop in &event_loops {
            if event_loop
                .send(ScriptThreadMessage::TickAllAnimations(Vec::new()))
                .is_err()
            {
                dead.push(event_loop.id());
            }
        }
        drop(event_loops);
        for id in &dead {
            self.forget_event_loop(*id);
        }
        let reason = format!("This page crashed. {which} disconnected ({error:?})");
        for id in dead {
            self.handle_panic(Some(id), reason.clone(), None);
        }
    }

    fn forget_event_loop(&mut self, id: ScriptEventLoopId) {
        if self
            .pinned_internal_event_loop
            .as_ref()
            .is_some_and(|pinned| pinned.id() == id)
        {
            self.pinned_internal_event_loop = None;
        }
        for webview in self.webviews.values() {
            webview
                .browsing_context_group()
                .borrow_mut()
                .event_loops
                .retain(|_, weak| match weak.upgrade() {
                    Some(event_loop) => event_loop.id() != id,
                    None => false,
                });
        }
    }

    fn handle_request_for_pipeline_namespace(&mut self, request: PipelineNamespaceRequest) {"""

OLD_SW_RECV = """                0 => oper
                    .recv(&self.sw_manager_receiver)
                    .expect("Unexpected service worker manager channel panic in constellation")
                    .map(Request::ServiceWorkerManager),"""

NEW_SW_RECV = """                0 => match oper.recv(&self.sw_manager_receiver) {
                    Ok(message) => message.map(Request::ServiceWorkerManager),
                    Err(error) => {
                        // Same class of disconnect as the script channel: the
                        // router closed, and panicking here aborts the browser.
                        // MITHU_PROCESS_CAP
                        warn!(
                            "service worker manager disconnected ({error:?}); continuing. MITHU_PROCESS_CAP"
                        );
                        self.sw_manager_receiver = crossbeam_channel::never();
                        return;
                    },
                },"""

MESSAGING = ROOT / "third_party/servo/components/script/messaging.rs"
OLD_SCRIPT_UNWRAP = """            } else if index == constellation_index {
                MixedMessage::FromConstellation(
                    operation
                        .recv(&self.constellation_receiver)
                        .unwrap()
                        .unwrap(),
                )"""

NEW_SCRIPT_UNWRAP = """            } else if index == constellation_index {
                // A closed constellation channel is the script thread being
                // asked to leave. Unwrapping that RecvError aborts the
                // process under panic=abort. MITHU_PROCESS_CAP
                let message = match operation.recv(&self.constellation_receiver) {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => {
                        log::warn!(
                            "constellation message failed ({error}); exiting script thread. MITHU_PROCESS_CAP"
                        );
                        ScriptThreadMessage::ExitScriptThread
                    },
                    Err(error) => {
                        log::warn!(
                            "constellation channel closed ({error}); exiting script thread. MITHU_PROCESS_CAP"
                        );
                        ScriptThreadMessage::ExitScriptThread
                    },
                };
                MixedMessage::FromConstellation(message)"""

FONT_SERVICE = ROOT / "third_party/servo/components/fonts/system_font_service.rs"
OLD_FONT_RECV = """        loop {
            let msg = self.port.recv().unwrap();"""

NEW_FONT_RECV = """        loop {
            // The constellation dropped this channel. Leave the thread
            // instead of panicking the process. MITHU_PROCESS_CAP
            let Ok(msg) = self.port.recv() else {
                break;
            };"""

OLD_CRASH_PAGE = "<p>Servo crashed!</p>"
NEW_CRASH_PAGE = "<p>This page crashed.</p>"
CRASH_PAGE = ROOT / "third_party/servo/components/default-resources/resources/crash.html"


def main() -> None:
    if not CONSTELLATION.is_file() or not EVENT_LOOP.is_file():
        return
    text = CONSTELLATION.read_text(encoding="utf-8")
    if MARKER not in text or "in_process: bool" not in EVENT_LOOP.read_text(encoding="utf-8"):
        replace_once(CONSTELLATION, OLD_BLANK, NEW_BLANK, "about:blank fallthrough")
        replace_once(CONSTELLATION, OLD_CREATE, NEW_CREATE, "event loop create")
        replace_once(EVENT_LOOP, OLD_SIG, NEW_SIG, "spawn signature")
        replace_once(EVENT_LOOP, OLD_SPAWN, NEW_SPAWN, "spawn branch")
    replace_once(CONSTELLATION, OLD_MONITOR, NEW_MONITOR, "hang monitor")
    replace_once(CONSTELLATION, OLD_FIELD, NEW_FIELD, "pinned event loop field")
    replace_once(CONSTELLATION, OLD_FIELD_INIT, NEW_FIELD_INIT, "pinned event loop init")
    replace_once(CONSTELLATION, OLD_SHUTDOWN, NEW_SHUTDOWN, "drop pinned event loop")
    replace_once(CONSTELLATION, OLD_HANDLER_ANCHOR, NEW_HANDLER_ANCHOR, "disconnect handler")
    replace_once(CONSTELLATION, OLD_NAMESPACE_RECV, NEW_NAMESPACE_RECV, "namespace channel")
    replace_any(
        CONSTELLATION,
        [OLD_SCRIPT_RECV, OLD_SCRIPT_RECV_SHUTDOWN],
        NEW_SCRIPT_RECV,
        "script channel",
    )
    replace_once(CONSTELLATION, OLD_BHM_RECV, NEW_BHM_RECV, "hang monitor channel")
    replace_once(CONSTELLATION, OLD_SW_RECV, NEW_SW_RECV, "service worker channel")
    replace_once(MESSAGING, OLD_SCRIPT_UNWRAP, NEW_SCRIPT_UNWRAP, "script thread exit")
    replace_once(FONT_SERVICE, OLD_FONT_RECV, NEW_FONT_RECV, "font service exit")
    replace_once(CRASH_PAGE, OLD_CRASH_PAGE, NEW_CRASH_PAGE, "crash page")
    replace_once(SERVO, OLD_JS, NEW_JS, "js engine")
    replace_once(SERVO, OLD_MEDIA, NEW_MEDIA, "media platform")


if __name__ == "__main__":
    main()
