#!/usr/bin/env python3
"""Share one in-process event loop for pages with no site, and cap site processes.

Servo starts a new content process whenever a document has no registrable
domain, so every about:blank tab becomes its own process. This patch keeps
those documents, and mithu: pages, on one in-process event loop. http(s)
sites still get a content process, and past four site processes a new site
reuses one that already exists. The Servo submodule itself is not forked.
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
    text = path.read_text(encoding="utf-8")
    if new in text:
        return
    if old not in text:
        sys.exit(f"{path.name}: could not find {label}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


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
    replace_once(SERVO, OLD_JS, NEW_JS, "js engine")
    replace_once(SERVO, OLD_MEDIA, NEW_MEDIA, "media platform")


if __name__ == "__main__":
    main()
