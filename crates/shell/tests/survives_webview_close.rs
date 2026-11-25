//! Closing a webview must not abort the process.
//!
//! The browser stays in-process here, which is the setup that used to drop
//! the shared script thread and close the constellation's script channel.
//! This file is still its own harness: if a content process is spawned, Servo
//! re-execs this binary with `--content-process`, and the stock unit-test
//! harness rejects that flag.

use std::process::ExitCode;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    mithu_shell::run_content_process_if_requested();
    mithu_host::suppress_dialog();
    mithu_shell::stay_in_process();

    let mut browser = match mithu_shell::Browser::try_new() {
        Ok(browser) => browser,
        Err(error) => {
            eprintln!("browser failed to start: {error}");
            return ExitCode::from(1);
        }
    };
    browser.resize(640.0, 480.0);
    browser.new_tab();
    if browser.tab_count() < 2 {
        eprintln!("expected two tabs, found {}", browser.tab_count());
        return ExitCode::from(1);
    }
    browser.navigate("data:text/html,<title>loading</title><p>loading</p>");
    let _ = browser.pump_scripts(Duration::from_millis(150));
    browser.close_tab();
    let _ = browser.pump_scripts(Duration::from_millis(400));
    if browser.tab_count() < 1 {
        eprintln!("closing a tab left no browser");
        return ExitCode::from(1);
    }
    browser.navigate("data:text/html,<title>alive</title><p>alive</p>");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) && !browser.load_settled() {
        let _ = browser.pump_scripts(Duration::from_millis(50));
    }
    let frame = browser.render(640.0, 480.0);
    if frame.is_empty() {
        eprintln!("the surviving tab painted nothing");
        return ExitCode::from(1);
    }
    // Drop shuts the constellation down. A disconnected script channel must
    // not panic out of this destructor.
    drop(browser);
    println!("survives_webview_close ok");
    ExitCode::SUCCESS
}
