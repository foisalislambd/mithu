//! testharness.js runner.
//!
//! A static server on 127.0.0.1 serves the WPT tree. HTML responses that
//! include testharness.js also include `/__mithu_report.js`, which registers
//! `add_completion_callback` and writes one record into `#__mithu_wpt`.
//! The shell loads each page, pumps scripts, and steps timers. Tests that
//! need wptserve substitutions (`{{...}}`) or a WebSocket server are skipped
//! and counted apart from the pass rate.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mithu_shell::Browser;

const REPORT_JS: &str = r#"
if (typeof Element !== "undefined" && !Element.prototype.insertAdjacentText) {
  Element.prototype.insertAdjacentText = function (where, text) {
    var node = document.createTextNode(String(text));
    if (where === "afterbegin") this.insertBefore(node, this.firstChild);
    else this.appendChild(node);
  };
}
function clean(value) {
  return String(value || "").replace(/[\t\n\r]/g, " ");
}
function mithuRecord(tests, status, phase) {
  var lines = [phase, String(status && status.status != null ? status.status : 0), String(status && status.message || "")];
  for (var i = 0; i < tests.length; i++) {
    var test = tests[i];
    lines.push(String(test.status) + "\t" + clean(test.name) + "\t" + clean(test.message));
  }
  var node = document.getElementById("__mithu_wpt");
  if (!node) {
    node = document.createElement("pre");
    node.setAttribute("id", "__mithu_wpt");
    (document.documentElement || document).appendChild(node);
  }
  node.textContent = lines.join("\n");
}
var mithuSeen = [];
add_result_callback(function (test) {
  mithuSeen.push(test);
  mithuRecord(mithuSeen, { status: 0, message: "" }, "partial");
});
add_completion_callback(function (tests, status) {
  try { mithuRecord(tests, status, "done"); } catch (error) {}
});
"#;

const DIRS: &[&str] = &[
    "dom",
    "html/dom",
    "html/semantics/scripting-1",
    "css/selectors",
    "css/css-flexbox",
    "css/css-grid",
    "fetch/api",
    "url",
    "encoding",
    "webstorage",
    "custom-elements",
    "shadow-dom",
    "intersection-observer",
    "web-animations",
    "svg/types",
    "xhr",
    "websockets",
    "workers",
];

struct Case {
    dir: String,
    /// URL path beginning with `/`, including the query when the file has variants.
    url_path: String,
    skip: Option<&'static str>,
}

struct Tally {
    pass: u64,
    fail: u64,
    timeout: u64,
    notrun: u64,
    precondition: u64,
    files: u64,
    skipped: Vec<(String, u64)>,
}

pub fn run(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        eprintln!("usage: mithu-measure wpt <wpt-root>");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(root);
    if !root.join("resources/testharness.js").is_file() {
        eprintln!("no resources/testharness.js under {}", root.display());
        return ExitCode::from(2);
    }
    let mut cases = collect(&root);
    let resume = args.get(1).cloned();
    let mut done = std::collections::HashSet::new();
    if let Some(path) = &resume {
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Some(url) = line.split('\t').next() {
                    done.insert(url.to_string());
                }
            }
            eprintln!("resume {} already recorded", done.len());
        }
    }
    cases.retain(|case| !done.contains(&case.url_path));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let root = Arc::new(root);
    let served = Arc::clone(&root);
    thread::spawn(move || serve(listener, served));
    eprintln!("wpt static server 127.0.0.1:{port} files {}", cases.len());

    let mut browser = Browser::new();
    browser.resize(800.0, 600.0);
    browser.set_cooperative_scripts(true);

    let mut by_dir: Vec<(String, Tally)> = DIRS
        .iter()
        .map(|dir| {
            (
                (*dir).to_string(),
                Tally {
                    pass: 0,
                    fail: 0,
                    timeout: 0,
                    notrun: 0,
                    precondition: 0,
                    files: 0,
                    skipped: Vec::new(),
                },
            )
        })
        .collect();

    let started = Instant::now();
    let mut ran_here = 0u32;
    for (index, case) in cases.iter().enumerate() {
        let tally = by_dir
            .iter_mut()
            .find(|item| item.0 == case.dir)
            .map(|item| &mut item.1);
        let Some(tally) = tally else {
            continue;
        };
        if let Some(reason) = case.skip {
            bump_skip(&mut tally.skipped, reason);
            if let Some(path) = &resume {
                append_line(path, &format!("{}\tskip\t{reason}\n", case.url_path));
            }
            continue;
        }
        let url = format!("http://127.0.0.1:{port}{}", case.url_path);
        if let Some(path) = &resume {
            let _ = fs::write(format!("{path}.current"), &case.url_path);
        }
        let record = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_one(&mut browser, &url)
        })) {
            Ok(record) => record,
            Err(_) => {
                eprintln!("crash {}", case.url_path);
                browser = Browser::new();
                browser.resize(800.0, 600.0);
                browser.set_cooperative_scripts(true);
                Record::Harness
            }
        };
        if let Some(path) = &resume {
            let line = match &record {
                Record::Subs(counts) => format!(
                    "{}\trun\t{}\t{}\t{}\t{}\t{}\n",
                    case.url_path, counts[0], counts[1], counts[2], counts[3], counts[4]
                ),
                Record::Harness => format!("{}\tharness\n", case.url_path),
            };
            append_line(path, &line);
        }
        tally.files += 1;
        match record {
            Record::Subs(counts) => {
                tally.pass += counts[0];
                tally.fail += counts[1];
                tally.timeout += counts[2];
                tally.notrun += counts[3];
                tally.precondition += counts[4];
            }
            Record::Harness => tally.fail += 1,
        }
        if index % 100 == 0 {
            eprintln!(
                "wpt {index}/{} {}s {}",
                cases.len(),
                started.elapsed().as_secs(),
                case.url_path
            );
        }
        // The shell keeps caches for the life of the process. Dropping it
        // every so often keeps the run inside memory.
        if case.skip.is_none() {
            ran_here += 1;
            if ran_here >= 120 {
                eprintln!("restart for memory");
                std::process::exit(2);
            }
        }
    }
    if let Some(path) = &resume {
        print_log(path);
        eprintln!("wpt done {}s", started.elapsed().as_secs());
        return ExitCode::SUCCESS;
    }
    println!("directory\tpass\tfail\ttimeout\tnotrun\tprecondition\tfiles\tskipped\tpct");
    for (dir, tally) in &by_dir {
        let total = tally.pass + tally.fail + tally.timeout + tally.notrun + tally.precondition;
        let pct = if total == 0 {
            0.0
        } else {
            tally.pass as f64 * 100.0 / total as f64
        };
        let skipped: u64 = tally.skipped.iter().map(|item| item.1).sum();
        let why: Vec<String> = tally
            .skipped
            .iter()
            .map(|(reason, count)| format!("{reason}={count}"))
            .collect();
        println!(
            "{dir}\t{}\t{}\t{}\t{}\t{}\t{}\t{skipped}\t{pct:.2}\t{}",
            tally.pass,
            tally.fail,
            tally.timeout,
            tally.notrun,
            tally.precondition,
            tally.files,
            why.join(",")
        );
    }
    eprintln!("wpt done {}s", started.elapsed().as_secs());
    ExitCode::SUCCESS
}

fn append_line(path: &str, line: &str) {
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn print_log(path: &str) {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut rows: Vec<(String, Tally)> = DIRS
        .iter()
        .map(|dir| {
            (
                (*dir).to_string(),
                Tally {
                    pass: 0,
                    fail: 0,
                    timeout: 0,
                    notrun: 0,
                    precondition: 0,
                    files: 0,
                    skipped: Vec::new(),
                },
            )
        })
        .collect();
    for line in text.lines() {
        let mut parts = line.split('\t');
        let Some(url) = parts.next() else { continue };
        let kind = parts.next().unwrap_or("");
        let dir = rows.iter().map(|row| row.0.clone()).find(|dir| {
            url.trim_start_matches("/__mithu_wrap__").trim_start_matches('/').starts_with(&format!("{dir}/"))
                || url.trim_start_matches("/__mithu_wrap__").trim_start_matches('/') == dir.trim_end_matches('/')
        });
        let Some(dir) = dir else { continue };
        let Some(tally) = rows.iter_mut().find(|row| row.0 == dir).map(|row| &mut row.1) else {
            continue;
        };
        match kind {
            "skip" => bump_skip(&mut tally.skipped, parts.next().unwrap_or("skip")),
            "run" => {
                tally.files += 1;
                let nums: Vec<u64> = parts.take(5).map(|part| part.parse().unwrap_or(0)).collect();
                if nums.len() == 5 {
                    tally.pass += nums[0];
                    tally.fail += nums[1];
                    tally.timeout += nums[2];
                    tally.notrun += nums[3];
                    tally.precondition += nums[4];
                }
            }
            "harness" => {
                tally.files += 1;
                tally.fail += 1;
            }
            _ => {}
        }
    }
    println!("directory\tpass\tfail\ttimeout\tnotrun\tprecondition\tfiles\tskipped\tpct");
    for (dir, tally) in &rows {
        let total = tally.pass + tally.fail + tally.timeout + tally.notrun + tally.precondition;
        let pct = if total == 0 { 0.0 } else { tally.pass as f64 * 100.0 / total as f64 };
        let skipped: u64 = tally.skipped.iter().map(|item| item.1).sum();
        let why: Vec<String> = tally.skipped.iter().map(|(reason, count)| format!("{reason}={count}")).collect();
        println!(
            "{dir}\t{}\t{}\t{}\t{}\t{}\t{}\t{skipped}\t{pct:.2}\t{}",
            tally.pass, tally.fail, tally.timeout, tally.notrun, tally.precondition, tally.files, why.join(",")
        );
    }
}

fn bump_skip(skipped: &mut Vec<(String, u64)>, reason: &str) {
    if let Some(slot) = skipped.iter_mut().find(|item| item.0 == reason) {
        slot.1 += 1;
    } else {
        skipped.push((reason.to_string(), 1));
    }
}

enum Record {
    /// Counts indexed by testharness status: PASS, FAIL, TIMEOUT, NOTRUN, PRECONDITION_FAILED.
    Subs([u64; 5]),
    Harness,
}

fn load_one(browser: &mut Browser, url: &str) -> Record {
    browser.navigate(url);
    let started = Instant::now();
    let mut steps = 0u32;
    let mut latest = None;
    // A long classic script yields on the instruction budget, and testharness
    // writes the result node after the first subtest. Keep pumping until the
    // completion callback runs, or the page goes idle, or the wall cap hits.
    loop {
        if let Some((done, record)) = read_record(browser) {
            if done {
                return record;
            }
            latest = Some(record);
        }
        let pending = browser.pump_scripts(Duration::from_millis(200));
        browser.advance_timers(0);
        if let Some((done, record)) = read_record(browser) {
            if done {
                return record;
            }
            latest = Some(record);
        }
        if started.elapsed() > Duration::from_secs(90) {
            return latest.unwrap_or(Record::Harness);
        }
        if !pending {
            let frames = browser.animation_frames_pending();
            match browser.next_timer_ms() {
                Some(ms) if steps < 200 => {
                    browser.advance_timers(ms.min(1_000));
                    steps += 1;
                    continue;
                }
                None if frames && steps < 200 => {
                    browser.advance_timers(16);
                    steps += 1;
                    continue;
                }
                _ => {
                    if std::env::var_os("MITHU_WPT_DEBUG").is_some() {
                        eprintln!(
                            "harness {} status {} logs {:?}",
                            url,
                            browser.status_line(),
                            browser.script_logs()
                        );
                    }
                    return latest.unwrap_or(Record::Harness);
                }
            }
        }
    }
}

/// `(completion callback ran, counts)`.
fn read_record(browser: &Browser) -> Option<(bool, Record)> {
    let text = browser.element_text("__mithu_wpt")?;
    let mut lines = text.lines();
    let phase = lines.next().unwrap_or("");
    let _status = lines.next().unwrap_or("");
    let _message = lines.next();
    let mut counts = [0u64; 5];
    let mut any = false;
    for line in lines {
        any = true;
        let status = line.split('\t').next().unwrap_or("1");
        let index = status.parse::<usize>().unwrap_or(1);
        if index < counts.len() {
            counts[index] += 1;
        } else {
            counts[1] += 1;
        }
    }
    if !any {
        counts[1] = 1;
    }
    Some((phase == "done", Record::Subs(counts)))
}

fn collect(root: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    for dir in DIRS {
        let base = root.join(dir);
        let mut files = Vec::new();
        walk(&base, &mut files);
        files.sort();
        for path in files {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.ends_with("-ref.html")
                || name.ends_with("-notref.html")
                || name.ends_with("-ref.htm")
                || name.ends_with("-ref.xhtml")
            {
                continue;
            }
            if rel.split('/').any(|part| part == "support" || part == "resources") {
                continue;
            }
            let Ok(body) = fs::read_to_string(&path) else {
                continue;
            };
            let lower_name = name.to_ascii_lowercase();
            let is_any = lower_name.ends_with(".any.js");
            let is_window = lower_name.ends_with(".window.js");
            let is_html = lower_name.ends_with(".html")
                || lower_name.ends_with(".htm")
                || lower_name.ends_with(".xhtml")
                || lower_name.ends_with(".xht");
            if !is_any && !is_window && !is_html {
                continue;
            }
            if is_html && !body.contains("testharness.js") {
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path: format!("/{rel}"),
                    skip: Some("not-testharness"),
                });
                continue;
            }
            if manual(&body) {
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path: format!("/{rel}"),
                    skip: Some("manual"),
                });
                continue;
            }
            if body.contains("{{") {
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path: format!("/{rel}"),
                    skip: Some("wptserve"),
                });
                continue;
            }
            if *dir == "websockets" && needs_socket(&body) {
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path: format!("/{rel}"),
                    skip: Some("server"),
                });
                continue;
            }
            if (is_any || is_window) && worker_only(&body) {
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path: format!("/{rel}"),
                    skip: Some("worker-only"),
                });
                continue;
            }
            let variants = variants_of(&body);
            if variants.is_empty() {
                let url_path = if is_any || is_window {
                    format!("/__mithu_wrap__/{rel}")
                } else {
                    format!("/{rel}")
                };
                cases.push(Case {
                    dir: (*dir).to_string(),
                    url_path,
                    skip: None,
                });
            } else {
                for variant in variants {
                    let url_path = if is_any || is_window {
                        format!("/__mithu_wrap__/{rel}{variant}")
                    } else {
                        format!("/{rel}{variant}")
                    };
                    cases.push(Case {
                        dir: (*dir).to_string(),
                        url_path,
                        skip: None,
                    });
                }
            }
        }
    }
    cases
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn manual(body: &str) -> bool {
    for line in body.lines().take(80) {
        let lower = line.to_ascii_lowercase();
        if lower.contains("meta:") && lower.contains("flags") && lower.contains("manual") {
            return true;
        }
        if lower.contains("name=\"flags\"") || lower.contains("name='flags'") || lower.contains("name=flags")
        {
            if lower.contains("manual") {
                return true;
            }
        }
    }
    false
}

fn needs_socket(body: &str) -> bool {
    body.contains("ws://")
        || body.contains("wss://")
        || body.contains("WebSocket")
        || body.contains(".py")
}

fn worker_only(body: &str) -> bool {
    let Some(line) = body.lines().take(40).find(|line| line.contains("META: global=")) else {
        return false;
    };
    let global = line.split("global=").nth(1).unwrap_or("");
    let global = global.split_whitespace().next().unwrap_or("");
    !global.split(',').any(|item| item == "window")
}

fn variants_of(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines().take(80) {
        if let Some(rest) = line.split("META: variant=").nth(1) {
            let variant = rest.trim();
            if variant.starts_with('?') {
                out.push(variant.to_string());
            }
        }
    }
    out
}

fn serve(listener: TcpListener, root: Arc<PathBuf>) {
    for stream in listener.incoming().flatten() {
        let root = Arc::clone(&root);
        thread::spawn(move || {
            let _ = handle(stream, &root);
        });
    }
}

fn handle(mut stream: TcpStream, root: &Path) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let decoded = decode(path);
    let (body, kind) = route(root, &decoded, query);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    Ok(())
}

fn route(root: &Path, path: &str, query: &str) -> (Vec<u8>, &'static str) {
    if path == "/__mithu_report.js" {
        return (REPORT_JS.as_bytes().to_vec(), "text/javascript");
    }
    let wrapped = path.strip_prefix("/__mithu_wrap__/");
    let rel = wrapped.unwrap_or(path.trim_start_matches('/'));
    if rel.contains("..") {
        return (b"bad path".to_vec(), "text/plain");
    }
    let file = root.join(rel);
    if wrapped.is_some() {
        let source = fs::read_to_string(&file).unwrap_or_default();
        let html = wrap_js(&source, rel, query);
        return (html.into_bytes(), "text/html; charset=utf-8");
    }
    let Ok(bytes) = fs::read(&file) else {
        return (b"missing".to_vec(), "text/plain");
    };
    let kind = if rel.ends_with(".js") || rel.ends_with(".mjs") {
        "text/javascript"
    } else if rel.ends_with(".css") {
        "text/css"
    } else if rel.ends_with(".html")
        || rel.ends_with(".htm")
        || rel.ends_with(".xhtml")
        || rel.ends_with(".xht")
    {
        "text/html; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    if kind.starts_with("text/html") {
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("testharness.js") {
            return (inject(&text).into_bytes(), kind);
        }
    }
    (bytes, kind)
}

fn wrap_js(source: &str, rel: &str, query: &str) -> String {
    let mut scripts = String::new();
    for line in source.lines().take(40) {
        if let Some(rest) = line.split("META: script=").nth(1) {
            let src = rest.trim();
            scripts.push_str(&format!("<script src=\"{src}\"></script>\n"));
        }
    }
    let _ = query;
    format!(
        "<!doctype html><meta charset=utf-8>\
<script src=\"/resources/testharness.js\"></script>\
<script src=\"/__mithu_report.js\"></script>\
{scripts}<script src=\"/{rel}\"></script>"
    )
}

fn inject(html: &str) -> String {
    let needle = "testharness.js";
    let Some(at) = html.find(needle) else {
        return html.to_string();
    };
    let after = &html[at + needle.len()..];
    let Some(end) = after.find("</script>") else {
        return html.to_string();
    };
    let cut = at + needle.len() + end + "</script>".len();
    let mut out = String::with_capacity(html.len() + 64);
    out.push_str(&html[..cut]);
    out.push_str("<script src=\"/__mithu_report.js\"></script>");
    out.push_str(&html[cut..]);
    out
}

fn decode(path: &str) -> String {
    let mut out = String::new();
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &path[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte as char);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}
