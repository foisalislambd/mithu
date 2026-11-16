//! Measurement harness. This is not part of the browser.
//!
//! `wpt` loads testharness pages through the shell and records
//! `add_completion_callback`. `test262` runs the TC39 suite in the
//! SpiderMonkey embedding.

mod test262;
mod wpt;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("usage: mithu-measure wpt <wpt-root> | test262 <test262-root> [--shard N/M]");
        return ExitCode::from(2);
    };
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "wpt" => wpt::run(&rest),
        "test262" => test262::run(&rest),
        other => {
            eprintln!("unknown command {other}");
            ExitCode::from(2)
        }
    }
}
