//! Lite must finish a local page load under a display. The glib loop
//! regression spun on Xvfb and logged `load_ms timeout` instead.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn local_page_and_internal_page_load_within_a_timeout() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("skip: no DISPLAY");
        return;
    }
    let port = serve_local_page();
    let root = std::env::temp_dir().join(format!("mithu-lite-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp profile");

    let local = run_lite(
        &root.join("http"),
        &[
            "--screenshot",
            root.join("local.png").to_str().unwrap(),
            &format!("http://127.0.0.1:{port}/"),
        ],
    );
    assert!(
        !local.contains("load_ms timeout"),
        "local page did not finish: {local}"
    );
    let ms = load_ms(&local);
    assert!(ms < 15_000, "local page took {ms} ms\n{local}");
    assert!(local.contains("page_url http://127.0.0.1:"));

    let internal = run_lite(
        &root.join("internal"),
        &[
            "--screenshot",
            root.join("downloads.png").to_str().unwrap(),
            "mithu://downloads",
        ],
    );
    assert!(
        !internal.contains("load_ms timeout"),
        "mithu://downloads did not finish: {internal}"
    );
    let ms = load_ms(&internal);
    assert!(ms < 15_000, "mithu://downloads took {ms} ms\n{internal}");
    assert!(
        internal.contains("page_url mithu://downloads"),
        "omnibox url missing: {internal}"
    );
}

fn serve_local_page() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = b"<html><head><title>Loaded</title></head><body>hello mithu</body></html>";
            let head = format!(
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    port
}

fn run_lite(profile: &std::path::Path, args: &[&str]) -> String {
    std::fs::create_dir_all(profile).expect("profile");
    // X11 auth lives in the real home. Point HOME at the profile so the
    // page store stays private, and keep the cookie on XAUTHORITY.
    let xauth = std::env::var_os("XAUTHORITY").unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        home.join(".Xauthority").into()
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_mithu-lite"))
        .args(args)
        .env("GDK_BACKEND", "x11")
        .env("XDG_DATA_HOME", profile)
        .env("XAUTHORITY", xauth)
        .env("HOME", profile)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mithu-lite");
    let started = Instant::now();
    let limit = Duration::from_secs(50);
    let status = loop {
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("mithu-lite did not exit within {}s", limit.as_secs());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => panic!("wait: {error}"),
        }
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    assert!(status.success(), "exit {status:?}\n{out}\n{err}");
    out
}

fn load_ms(output: &str) -> u128 {
    output
        .lines()
        .find_map(|line| line.strip_prefix("load_ms "))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no load_ms in {output}"))
}
