//! Full test262 run on Mithu's SpiderMonkey embedding.
//!
//! Each file is a fresh page. `assert.js` and `sta.js` are prepended unless
//! the test is `raw`. A test with neither `onlyStrict` nor `noStrict` runs
//! twice, once in sloppy mode and once in strict mode, and passes only when
//! both pass. `negative` tests pass when the thrown error name matches.
//! Async tests pass when `$DONE()` runs with no error.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use mithu_html::parse_html;
use mithu_js::Session;

struct Meta {
    includes: Vec<String>,
    flags: Vec<String>,
    negative_phase: Option<String>,
    negative_type: Option<String>,
}

pub fn run(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        eprintln!("usage: mithu-measure test262 <test262-root> [--shard N/M]");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(root);
    let (shard, shards) = shard_of(&args[1..]);
    let harness = root.join("harness");
    let assert_js = fs::read_to_string(harness.join("assert.js")).unwrap_or_default();
    let sta_js = fs::read_to_string(harness.join("sta.js")).unwrap_or_default();
    let mut files = Vec::new();
    walk(&root.join("test"), &mut files);
    files.sort();
    let mine: Vec<_> = files
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % shards == shard)
        .map(|(_, path)| path)
        .collect();
    eprintln!(
        "test262 shard {}/{} files {}",
        shard + 1,
        shards,
        mine.len()
    );
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut passed = 0u64;
    let mut failed = 0u64;
    let started = Instant::now();
    for (index, path) in mine.iter().enumerate() {
        let Ok(source) = fs::read_to_string(path) else {
            failed += 1;
            continue;
        };
        let meta = parse_meta(&source);
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            modes(&meta).into_iter().all(|strict| {
                execute(
                    &harness,
                    &assert_js,
                    &sta_js,
                    &source,
                    &meta,
                    strict,
                    &mut cache,
                )
            })
        }));
        match ok {
            Ok(true) => passed += 1,
            _ => failed += 1,
        }
        if index % 500 == 0 {
            eprintln!(
                "test262 shard {} {passed} pass {failed} fail {}s",
                shard + 1,
                started.elapsed().as_secs()
            );
        }
    }
    println!(
        "summary shard={shard} shards={shards} pass={passed} fail={failed} total={}",
        passed + failed
    );
    ExitCode::SUCCESS
}

fn shard_of(args: &[String]) -> (usize, usize) {
    let Some(flag) = args.iter().find(|arg| arg.starts_with("--shard=")) else {
        return (0, 1);
    };
    let spec = flag.trim_start_matches("--shard=");
    let Some((left, right)) = spec.split_once('/') else {
        return (0, 1);
    };
    let shard = left.parse::<usize>().unwrap_or(0);
    let shards = right.parse::<usize>().unwrap_or(1).max(1);
    (shard.min(shards - 1), shards)
}

fn modes(meta: &Meta) -> Vec<bool> {
    if meta.flags.iter().any(|flag| flag == "module" || flag == "raw") {
        return vec![meta.flags.iter().any(|flag| flag == "module" || flag == "onlyStrict")];
    }
    let only = meta.flags.iter().any(|flag| flag == "onlyStrict");
    let sloppy = meta.flags.iter().any(|flag| flag == "noStrict");
    if only && !sloppy {
        vec![true]
    } else if sloppy && !only {
        vec![false]
    } else {
        vec![false, true]
    }
}

fn execute(
    harness: &Path,
    assert_js: &str,
    sta_js: &str,
    source: &str,
    meta: &Meta,
    strict: bool,
    cache: &mut HashMap<String, String>,
) -> bool {
    let raw = meta.flags.iter().any(|flag| flag == "raw");
    let module = meta.flags.iter().any(|flag| flag == "module");
    let async_test = meta.flags.iter().any(|flag| flag == "async");
    let mut code = String::new();
    if strict && !module {
        code.push_str("\"use strict\";\n");
    }
    code.push_str(
        "var __mithuDone = false; function $DONE(error) { if (__mithuDone) return; __mithuDone = true; if (error) { document.title = 'fail:' + (error.name || error); } else { document.title = 'pass'; } }\n",
    );
    if !raw {
        code.push_str(assert_js);
        code.push('\n');
        code.push_str(sta_js);
        code.push('\n');
    }
    for include in &meta.includes {
        let text = cache.entry(include.clone()).or_insert_with(|| {
            fs::read_to_string(harness.join(include)).unwrap_or_default()
        });
        code.push_str(text);
        code.push('\n');
    }
    let body = source.replace("</script", "<\\/script");
    code.push_str(&body);
    let kind = if module { " type=module" } else { "" };
    let html = format!("<script{kind}>\n{code}\n</script>");
    let doc = parse_html(&html);
    let mut session = Session::open_cooperative(
        doc,
        "https://test262.test/test",
        |_| None,
        HashMap::new(),
        HashMap::new(),
    );
    let started = Instant::now();
    while session.scripts_pending() && started.elapsed() < Duration::from_secs(2) {
        session.pump(Duration::from_millis(250));
    }
    if async_test {
        for _ in 0..30 {
            if session.document().title.starts_with("pass")
                || session.document().title.starts_with("fail:")
            {
                break;
            }
            session.advance(16);
        }
    } else {
        session.advance(0);
    }
    let title = session.document().title.clone();
    let logs = session.logs().join("\n");
    let negative = meta.negative_type.as_deref();
    if async_test {
        if title == "pass" && logs.is_empty() {
            return negative.is_none();
        }
        if let Some(expected) = negative {
            return title.contains(expected) || logs.contains(expected);
        }
        return false;
    }
    if let Some(expected) = negative {
        let early = meta.negative_phase.as_deref() == Some("parse");
        let matched = logs.contains(expected) || title.contains(expected);
        if early {
            return matched;
        }
        return matched && title != "pass";
    }
    title != "fail" && !title.starts_with("fail:") && logs.is_empty() && !session.scripts_pending()
}

fn parse_meta(source: &str) -> Meta {
    let mut meta = Meta {
        includes: Vec::new(),
        flags: Vec::new(),
        negative_phase: None,
        negative_type: None,
    };
    let Some(start) = source.find("/*---") else {
        return meta;
    };
    let rest = &source[start + 5..];
    let Some(end) = rest.find("---*/") else {
        return meta;
    };
    let head = &rest[..end];
    let mut section = "";
    for line in head.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("includes:") {
            section = "includes";
            push_list(&mut meta.includes, rest);
        } else if let Some(rest) = line.strip_prefix("flags:") {
            section = "flags";
            push_list(&mut meta.flags, rest);
        } else if line.starts_with("negative:") {
            section = "negative";
        } else if let Some(rest) = line.strip_prefix("phase:") {
            if section == "negative" {
                meta.negative_phase = Some(clean(rest));
            }
        } else if let Some(rest) = line.strip_prefix("type:") {
            if section == "negative" {
                meta.negative_type = Some(clean(rest));
            }
        } else if let Some(rest) = line.strip_prefix("- ") {
            let item = clean(rest);
            if item.is_empty() {
                continue;
            }
            match section {
                "includes" => meta.includes.push(item),
                "flags" => meta.flags.push(item),
                _ => {}
            }
        } else if line.ends_with(':') && !line.starts_with('-') {
            section = "";
        }
    }
    meta
}

fn push_list(out: &mut Vec<String>, rest: &str) {
    let rest = rest.trim();
    if !rest.starts_with('[') {
        return;
    }
    let inner = rest.trim_start_matches('[').trim_end_matches(']');
    for part in inner.split(',') {
        let item = clean(part);
        if !item.is_empty() {
            out.push(item);
        }
    }
}

fn clean(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches(',')
        .to_string()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "js") {
            out.push(path);
        }
    }
}
