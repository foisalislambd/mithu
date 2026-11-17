//! `mithu://` pages shared by Mithu and Mithu Lite.
//!
//! The documents are generated here so both editions show the same settings,
//! history, downloads, bookmarks, and new tab. Nothing in the markup names
//! another browser or uses its marks.

use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    /// When set, pages point at `logo.png`, `mark.png`, and `fonts/*.woff2`
    /// instead of embedding those bytes. The phone shells copy the files
    /// beside the HTML. Desktop pages stay self-contained.
    static FILE_ASSETS: Cell<bool> = const { Cell::new(false) };
}

/// Phone builds call this around `open` so the saved HTML stays small.
pub fn use_file_assets(enabled: bool) {
    FILE_ASSETS.with(|cell| cell.set(enabled));
}

fn file_assets() -> bool {
    FILE_ASSETS.with(|cell| cell.get())
}

use url::Url;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub theme: String,
    pub font_px: u32,
    pub zoom: f32,
    pub show_home: bool,
    pub show_bookmarks: bool,
    pub search: String,
    pub startup: String,
    pub startup_pages: Vec<String>,
    pub download_dir: String,
    pub languages: String,
    pub ask_where: bool,
    pub smooth_scroll: bool,
    pub hardware: bool,
    pub name: String,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<Visit>,
    pub downloads: Vec<Download>,
    /// Site permission id and choice: ask, allow, or block. This is the default.
    pub site: Vec<(String, String)>,
    /// Per-origin exceptions. The engine uses these before the default.
    pub exceptions: Vec<Exception>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exception {
    pub origin: String,
    pub feature: String,
    pub choice: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub title: String,
    pub url: String,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub url: String,
    pub path: String,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub url: String,
    pub title: String,
    pub html: String,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub redirect: String,
    pub open_path: Option<String>,
    pub reveal_path: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "light".into(),
            font_px: 16,
            zoom: 1.0,
            show_home: true,
            show_bookmarks: true,
            search: "duckduckgo".into(),
            startup: "newtab".into(),
            startup_pages: Vec::new(),
            download_dir: String::new(),
            languages: "English".into(),
            ask_where: false,
            smooth_scroll: true,
            hardware: true,
            name: "Mithu".into(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            downloads: Vec::new(),
            site: Vec::new(),
            exceptions: Vec::new(),
        }
    }
}

impl Prefs {
    pub fn dark(&self) -> bool {
        self.theme == "dark"
    }

    pub fn remember(&mut self, url: &str, title: &str) {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return;
        }
        if self.history.last().is_some_and(|visit| visit.url == url) {
            return;
        }
        self.history.push(Visit {
            title: if title.is_empty() { url.to_string() } else { title.to_string() },
            url: url.to_string(),
            at: now_secs(),
        });
        if self.history.len() > 500 {
            let drop_n = self.history.len() - 500;
            self.history.drain(0..drop_n);
        }
    }

    pub fn note_download(&mut self, url: &str, path: &str, state: &str) {
        if let Some(item) = self.downloads.iter_mut().find(|item| item.url == url && item.state != "done") {
            item.path = path.to_string();
            item.state = state.to_string();
            return;
        }
        self.downloads.push(Download {
            url: url.to_string(),
            path: path.to_string(),
            state: state.to_string(),
        });
    }
}

pub fn directory() -> PathBuf {
    let base = if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(base).join("Mithu")
    } else {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join(".local").join("share").join("Mithu")
    };
    if std::env::var_os("MITHU_PRIVATE").is_some() {
        base.join("Private")
    } else {
        base
    }
}

pub fn path() -> PathBuf {
    directory().join("prefs.mithu")
}

/// Folder used when the user has not chosen one in settings.
pub fn downloads_folder() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = home.join("Downloads");
    if std::fs::create_dir_all(&dir).is_ok() {
        dir
    } else {
        std::env::temp_dir()
    }
}

pub fn load(path: &Path) -> Prefs {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Prefs::default();
    };
    parse_prefs(&text)
}

pub fn save(path: &Path, prefs: &Prefs) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format_prefs(prefs));
}

/// DuckDuckGo, Bing, or Wikipedia. The default stays DuckDuckGo.
pub fn search_url(engine: &str, query: &str) -> Url {
    let q = percent(query);
    let spec = match engine {
        "bing" => format!("https://www.bing.com/search?q={q}"),
        "wikipedia" => format!("https://en.wikipedia.org/w/index.php?search={q}"),
        _ => format!("https://duckduckgo.com/?q={q}"),
    };
    Url::parse(&spec).expect("search")
}

/// Save a remote page into `dir`. The name comes from the URL path.
pub fn save_remote(url: &str, dir: &Path) -> Result<PathBuf, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("This page cannot be saved.".into());
    }
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(20 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let path = unique_file(dir, &download_name(url));
    std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok(path)
}

fn download_name(url: &str) -> String {
    let leaf = Url::parse(url).ok().and_then(|url| {
        url.path_segments().and_then(|segments| segments.filter(|part| !part.is_empty()).last().map(str::to_string))
    });
    let name = leaf.unwrap_or_else(|| "page.html".into());
    let cleaned: String = name.chars().map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') { ch } else { '_' }).collect();
    if cleaned.is_empty() { "page.html".into() } else { cleaned }
}

fn unique_file(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(name).file_stem().and_then(|stem| stem.to_str()).unwrap_or("page");
    let ext = Path::new(name).extension().and_then(|ext| ext.to_str()).unwrap_or("html");
    for index in 1..1000 {
        let path = dir.join(format!("{stem}-{index}.{ext}"));
        if !path.exists() {
            return path;
        }
    }
    dir.join(name)
}

pub fn is_internal(spec: &str) -> bool {
    classify(spec).is_some()
}

pub fn open(spec: &str, prefs: &Prefs, version: &str) -> Option<Document> {
    let kind = classify(spec)?;
    Some(render(kind, spec, prefs, version))
}

pub fn command(spec: &str, prefs: &mut Prefs) -> Option<Command> {
    let url = Url::parse(spec).ok()?;
    if url.scheme() != "mithu" || url.host_str() != Some("do") {
        return None;
    }
    let query = spec.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut op = String::new();
    let mut back = "mithu://settings".to_string();
    let mut key = String::new();
    let mut value = String::new();
    let mut extra: Vec<(String, String)> = Vec::new();
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        let v = decode(v);
        match k {
            "op" => op = v,
            "back" => back = v,
            "key" => key = v,
            "value" => value = v,
            other => extra.push((other.to_string(), v)),
        }
    }
    let mut open_path = None;
    let mut reveal_path = None;
    match op.as_str() {
        "set" => apply_set(prefs, &key, &value),
        "startup-page" => {
            if !value.trim().is_empty() {
                prefs.startup_pages.push(value.trim().to_string());
                prefs.startup = "pages".into();
            }
        }
        "forget" => prefs.history.retain(|visit| visit.url != value),
        "forget-all" => prefs.history.clear(),
        "forget-query" => {
            let needle = value.to_ascii_lowercase();
            prefs.history.retain(|visit| {
                !visit.url.to_ascii_lowercase().contains(&needle) && !visit.title.to_ascii_lowercase().contains(&needle)
            });
        }
        "download" => {
            if let Some(item) = prefs.downloads.iter_mut().find(|item| item.url == key || item.path == key) {
                match value.as_str() {
                    "open" => open_path = Some(item.path.clone()),
                    "reveal" => reveal_path = Some(item.path.clone()),
                    "cancel" => item.state = "canceled".into(),
                    "retry" => item.state = "progressing".into(),
                    "done" => item.state = "done".into(),
                    _ => {}
                }
            }
        }
        "bookmark" => {
            let title = field(&extra, "title");
            let folder = {
                let folder = field(&extra, "folder");
                if folder.is_empty() { "Bookmarks bar".into() } else { folder }
            };
            if !value.is_empty() {
                if let Some(mark) = prefs.bookmarks.iter_mut().find(|mark| mark.url == value) {
                    if !title.is_empty() {
                        mark.title = title;
                    }
                    mark.folder = folder;
                } else {
                    prefs.bookmarks.push(Bookmark {
                        title: if title.is_empty() { value.clone() } else { title },
                        url: value,
                        folder,
                    });
                }
            }
        }
        "bookmark-del" => prefs.bookmarks.retain(|mark| mark.url != value),
        "import" => import_html(prefs, &value),
        "reset" => {
            let bookmarks = prefs.bookmarks.clone();
            let history = prefs.history.clone();
            let downloads = prefs.downloads.clone();
            *prefs = Prefs::default();
            prefs.bookmarks = bookmarks;
            prefs.history = history;
            prefs.downloads = downloads;
        }
        _ => return None,
    }
    if !back.starts_with("mithu://") {
        back = "mithu://settings".into();
    }
    Some(Command { redirect: back, open_path, reveal_path })
}

fn field(pairs: &[(String, String)], name: &str) -> String {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default()
}

fn apply_set(prefs: &mut Prefs, key: &str, value: &str) {
    match key {
        "theme" if value == "light" || value == "dark" => prefs.theme = value.into(),
        "font_px" => {
            if let Ok(px) = value.parse::<u32>() {
                prefs.font_px = px.clamp(12, 24);
            }
        }
        "zoom" => {
            if let Ok(zoom) = value.parse::<f32>() {
                prefs.zoom = zoom.clamp(0.5, 2.0);
            }
        }
        "show_home" => prefs.show_home = value != "0" && value != "false",
        "show_bookmarks" => prefs.show_bookmarks = value != "0" && value != "false",
        "search" if matches!(value, "duckduckgo" | "bing" | "wikipedia") => prefs.search = value.into(),
        "startup" if matches!(value, "newtab" | "continue" | "pages") => prefs.startup = value.into(),
        "download_dir" => prefs.download_dir = value.into(),
        "languages" => prefs.languages = value.into(),
        "ask_where" => prefs.ask_where = value != "0" && value != "false",
        "smooth_scroll" => prefs.smooth_scroll = value != "0" && value != "false",
        "hardware" => prefs.hardware = value != "0" && value != "false",
        "name" => {
            if !value.trim().is_empty() {
                prefs.name = value.trim().to_string();
            }
        }
        "exception" => set_exception(prefs, value),
        "perm" => {
            let Some((id, choice)) = value.split_once(':') else { return };
            if !matches!(choice, "ask" | "allow" | "block") || !site_known(id) {
                return;
            }
            if let Some(row) = prefs.site.iter_mut().find(|(key, _)| key == id) {
                row.1 = choice.to_string();
            } else {
                prefs.site.push((id.to_string(), choice.to_string()));
            }
        }
        _ => {}
    }
}

fn import_html(prefs: &mut Prefs, html: &str) {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let raw = html;
    let mut offset = 0;
    while let Some(rel) = rest.find("<a ") {
        let start = offset + rel;
        let after = &raw[start..];
        let end = after.find("</a>").unwrap_or(0);
        let tag = &after[..end.min(after.len())];
        if let Some(url) = attr(tag, "href") {
            if url.starts_with("http://") || url.starts_with("https://") {
                let title = after.get(tag.len() + 4..end).unwrap_or("").trim();
                let title = strip_tags(title);
                if !prefs.bookmarks.iter().any(|mark| mark.url == url) {
                    prefs.bookmarks.push(Bookmark {
                        title: if title.is_empty() { url.clone() } else { title },
                        url,
                        folder: "Imported".into(),
                    });
                }
            }
        }
        let step = rel + 3;
        rest = &rest[step..];
        offset += step;
    }
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.to_ascii_lowercase().find(&needle)? + needle.len();
    let rest = tag.get(start..)?;
    let end = rest.find('"')?;
    Some(decode_basic(&rest[..end]))
}

#[cfg(test)]
fn strip_data_uris(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find("data:") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('"').unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn strip_tags(text: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for ch in text.chars() {
        match ch {
            '<' => skip = true,
            '>' => skip = false,
            _ if !skip => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn decode_basic(text: &str) -> String {
    text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"")
}

#[derive(Clone, Copy)]
enum Kind {
    Settings,
    History,
    Downloads,
    Bookmarks,
    Extensions,
    NewTab,
    Version,
    Translate,
    Inspect,
    Tasks,
}

fn classify(spec: &str) -> Option<Kind> {
    let spec = spec.trim();
    if spec == "mithu://do" || spec.starts_with("mithu://do?") {
        return None;
    }
    let url = Url::parse(spec).ok()?;
    let host = url.host_str().unwrap_or("");
    let path = url.path().trim_matches('/');
    match (url.scheme(), host, path) {
        ("mithu", "settings", _) | ("about", "", "settings") => Some(Kind::Settings),
        ("mithu", "history", _) | ("about", "", "history") => Some(Kind::History),
        ("mithu", "downloads", _) | ("about", "", "downloads") => Some(Kind::Downloads),
        ("mithu", "bookmarks", _) => Some(Kind::Bookmarks),
        ("mithu", "extensions", _) => Some(Kind::Extensions),
        ("mithu", "newtab", _) | ("about", "", "newtab" | "home" | "mithu") => Some(Kind::NewTab),
        ("mithu", "version", _) => Some(Kind::Version),
        ("mithu", "translate", _) => Some(Kind::Translate),
        ("mithu", "inspect", _) => Some(Kind::Inspect),
        ("mithu", "tasks", _) => Some(Kind::Tasks),
        _ => None,
    }
}

fn render(kind: Kind, spec: &str, prefs: &Prefs, version: &str) -> Document {
    let (url, title, body) = match kind {
        Kind::Settings => ("mithu://settings", "Settings", settings_body(spec, prefs)),
        Kind::History => ("mithu://history", "History", history_body(spec, prefs)),
        Kind::Downloads => ("mithu://downloads", "Downloads", downloads_body(spec, prefs)),
        Kind::Bookmarks => ("mithu://bookmarks", "Bookmarks", bookmarks_body(spec, prefs)),
        Kind::Extensions => ("mithu://extensions", "Extensions", extensions_body(spec)),
        Kind::NewTab => ("mithu://newtab", "New Tab", newtab_body(prefs)),
        Kind::Version => ("mithu://version", "Version", version_body(version)),
        Kind::Translate => ("mithu://translate", "Translate", translate_body()),
        Kind::Inspect => ("mithu://inspect", "Developer tools", inspect_body()),
        Kind::Tasks => ("mithu://tasks", "Task manager", task_body(prefs)),
    };
    Document {
        url: url.into(),
        title: title.into(),
        html: shell(title, prefs, version, &body),
    }
}

fn roboto_faces() -> String {
    if file_assets() {
        return "@font-face{font-family:Roboto;font-style:normal;font-weight:400;font-display:swap;src:url(fonts/roboto-regular.woff2) format('woff2');}\
@font-face{font-family:Roboto;font-style:normal;font-weight:500;font-display:swap;src:url(fonts/roboto-medium.woff2) format('woff2');}\
@font-face{font-family:Roboto;font-style:normal;font-weight:700;font-display:swap;src:url(fonts/roboto-bold.woff2) format('woff2');}"
            .into();
    }
    fn face(weight: u16, bytes: &[u8]) -> String {
        format!(
            "@font-face{{font-family:Roboto;font-style:normal;font-weight:{weight};font-display:swap;src:url(data:font/woff2;base64,{}) format('woff2');}}",
            b64(bytes)
        )
    }
    format!(
        "{}{}{}",
        face(400, include_bytes!("../../../assets/fonts/roboto/roboto-regular.woff2")),
        face(500, include_bytes!("../../../assets/fonts/roboto/roboto-medium.woff2")),
        face(700, include_bytes!("../../../assets/fonts/roboto/roboto-bold.woff2")),
    )
}

fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let n = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8) | bytes[index + 2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        index += 3;
    }
    let left = bytes.len() - index;
    if left == 1 {
        let n = (bytes[index] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if left == 2 {
        let n = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

fn mark_src() -> String {
    if file_assets() {
        "mark.png".into()
    } else {
        crate::brand::mark_data_uri()
    }
}

fn logo_src() -> String {
    if file_assets() {
        "logo.png".into()
    } else {
        crate::brand::logo_data_uri()
    }
}

fn icon_src() -> String {
    if file_assets() {
        "mark.png".into()
    } else {
        crate::brand::favicon_data_uri()
    }
}

fn shell(title: &str, prefs: &Prefs, version: &str, body: &str) -> String {
    let theme = if prefs.dark() { "dark" } else { "light" };
    format!(
        r#"<!DOCTYPE html>
<html lang="en" data-theme="{theme}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — Mithu</title>
<link rel="icon" href="{icon}">
<style>
{fonts}
:root {{
  color-scheme: light;
  --bg: #fff;
  --card: #fff;
  --ink: #1f1f1f;
  --muted: #5f6368;
  --line: rgba(0, 0, 0, .06);
  --hairline: rgba(0, 0, 0, .14);
  --field: #f1f3f4;
  --accent: #0b57d0;
  --accent-ink: #fff;
  --side: #fff;
  --nav: #202124;
  --nav-selected: #1967d2;
  --nav-bg: #e8f0fe;
  --hover: #e8eaed;
  --danger: #d93025;
}}
html[data-theme="dark"] {{
  color-scheme: dark;
  --bg: #1f1f1f;
  --card: #292a2d;
  --ink: #e3e3e3;
  --muted: #9aa0a6;
  --line: rgba(255, 255, 255, .1);
  --hairline: rgba(255, 255, 255, .14);
  --field: #3c3c3c;
  --accent: #a8c7fa;
  --accent-ink: #062e6f;
  --side: #1f1f1f;
  --nav: #fff;
  --nav-selected: #202124;
  --nav-bg: #8ab4f8;
  --hover: #3c4043;
  --danger: #f28b82;
}}
* {{ box-sizing: border-box; }}
html, body {{ margin:0; background:var(--bg); color:var(--ink); font: {font}px/1.54 Roboto, "Noto Sans", Arial, sans-serif; }}
a {{ color: var(--accent); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
button, input, select, textarea {{ font: inherit; color: inherit; }}
.markimg {{ width:24px; height:24px; border-radius:50%; }}
img.logo {{ width:min(240px, 70vw); height:auto; display:block; margin:0 auto 8px; }}
.app {{ display:flex; min-height:100vh; }}
aside {{ width:256px; flex:none; padding:8px 0 24px; }}
.pagetitle {{ display:flex; align-items:center; gap:12px; height:48px; padding:8px 20px 0; font-size:22px; font-weight:400; }}
.maincol {{ flex:1; min-width:0; display:flex; flex-direction:column; }}
.searchrow {{ height:56px; display:flex; align-items:center; padding:8px 24px 0; }}
.search {{ width:min(720px, 100%); height:40px; background:var(--field); border:0; border-radius:20px; padding:0 16px 0 16px; }}
nav a, .folders a {{ display:flex; align-items:center; gap:12px; min-height:40px; margin:0 8px 0 0; padding:0 16px 0 20px; border-radius:0 100px 100px 0; color:var(--nav); font-size:13px; font-weight:500; text-decoration:none; }}
nav a svg {{ width:20px; height:20px; flex:none; fill:none; stroke:currentColor; stroke-width:1.6; stroke-linecap:round; stroke-linejoin:round; }}
nav a:hover, .folders a:hover {{ background:var(--hover); }}
nav a.on, .folders a.on {{ background:var(--nav-bg); color:var(--nav-selected); }}
main {{ flex:1; padding:8px 24px 64px; max-width:760px; }}
h1 {{ font-size:15px; font-weight:400; line-height:1.4; margin:12px 0 12px; color:var(--muted); }}
h2 {{ font-size:13px; font-weight:500; margin:16px 0 8px; color:var(--muted); }}
section.on > h2 {{ display:none; }}
.lead {{ color:var(--muted); margin:0 0 16px; font-size:13px; }}
.card {{ background:var(--card); border:1px solid var(--hairline); border-radius:8px; padding:0 16px; margin:0 0 16px; }}
.row {{ display:flex; align-items:center; gap:16px; min-height:48px; border-top:1px solid var(--line); padding:8px 0; }}
.card .row:first-child {{ border-top:0; }}
.row .grow {{ flex:1; }}
.row small {{ display:block; color:var(--muted); font-size:12px; font-weight:400; }}
select, .text {{ background:var(--field); border:0; border-radius:4px; padding:8px 10px; min-height:36px; }}
.btn {{ border:0; background:var(--accent); color:var(--accent-ink); border-radius:18px; min-height:36px; padding:0 16px; cursor:pointer; font-weight:500; }}
.btn.quiet {{ background:transparent; color:var(--accent); border:1px solid var(--hairline); }}
.btn.danger {{ background:transparent; color:var(--danger); border:1px solid var(--danger); }}
.tiles {{ display:grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap:12px; }}
.tile {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:16px; min-height:88px; }}
.center {{ min-height:70vh; display:grid; place-items:center; text-align:center; }}
.hero {{ width:72px; height:72px; border-radius:20px; background:var(--accent); color:var(--accent-ink); display:grid; place-items:center; font-size:32px; font-weight:700; margin:0 auto 12px; }}
table {{ width:100%; border-collapse:collapse; }}
td {{ padding:10px 8px; border-top:1px solid var(--line); vertical-align:top; }}
.hidden {{ display:none !important; }}
.solo-title {{ display:none; align-items:center; gap:12px; font-size:22px; margin-right:16px; white-space:nowrap; }}
.app.solo aside {{ display:none; }}
.app.solo .solo-title {{ display:flex; }}
.app.solo .searchrow {{ padding-left:20px; }}
.app.solo main {{ max-width:none; }}
.empty {{ min-height:50vh; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:12px; color:var(--muted); text-align:center; }}
.tabs {{ display:flex; gap:16px; border-bottom:1px solid var(--line); margin-bottom:16px; }}
.tabs a {{ color:var(--muted); padding:10px 4px; font-weight:500; font-size:13px; }}
.tabs a.on {{ color:var(--accent); box-shadow: inset 0 -2px 0 var(--accent); }}
nav .sep {{ height:1px; margin:8px 16px; background:var(--line); }}
.day {{ font-size:15px; font-weight:500; color:var(--ink); margin:8px 0 8px; }}
.hrow {{ display:flex; align-items:center; min-height:36px; gap:8px; padding:0 14px; }}
.hrow + .hrow {{ box-shadow: inset 0 1px 0 var(--line); }}
.htime {{ width:64px; flex:none; color:#646464; font-size:13px; }}
html[data-theme="dark"] .htime {{ color:#9aa0a6; }}
.favicon {{ width:16px; height:16px; border-radius:2px; background:var(--field); flex:none; }}
.hmain {{ flex:1; min-width:0; }}
.hmain a {{ color:var(--ink); font-size:13px; }}
.hmain small {{ display:block; color:var(--muted); font-size:12px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }}
.dl {{ display:flex; align-items:center; gap:16px; min-height:64px; padding:8px 0; border-top:1px solid var(--line); }}
.dl:first-child {{ border-top:0; }}
.dl .grow a {{ color:var(--accent); font-weight:500; }}
.fileico {{ width:24px; height:24px; border-radius:4px; background:var(--field); flex:none; }}
.sw {{ width:36px; height:20px; border-radius:10px; border:0; background:#bdc1c6; position:relative; padding:0; cursor:pointer; flex:none; }}
.sw.on {{ background:var(--accent); }}
.sw i {{ display:block; width:16px; height:16px; border-radius:50%; background:#fff; position:absolute; top:2px; left:2px; }}
.sw.on i {{ left:18px; }}
.aboutbox {{ text-align:center; padding:32px 0 8px; }}
.aboutbox .logo {{ margin-bottom:12px; }}
.sub {{ color:var(--muted); font-size:13px; margin:4px 0 16px; }}
</style>
</head>
<body>
<div class="app {layout}">
<aside>
<div class="pagetitle"><img class="markimg" src="{mark}" alt="">{page_title}</div>
<nav>
{nav}
</nav>
</aside>
<div class="maincol">
<div class="searchrow"><div class="solo-title"><img class="markimg" src="{mark}" alt="">{page_title}</div><input class="search" id="q" placeholder="{search_label}" aria-label="{search_label}"></div>
<main>{body}</main>
</div>
</div>
<script>
var q = document.getElementById("q");
q.addEventListener("input", function () {{
  var needle = q.value.toLowerCase();
  document.querySelectorAll("[data-section]").forEach(function (node) {{
    var hay = (node.getAttribute("data-section") || "").toLowerCase();
    var show = needle ? hay.indexOf(needle) >= 0 : node.classList.contains("on");
    node.classList.toggle("hidden", !show);
  }});
}});
q.addEventListener("keydown", function (event) {{
  if (event.key !== "Enter") return;
  var page = location.href;
  var key = page.indexOf("history") >= 0 ? "mithu://history?q=" : page.indexOf("downloads") >= 0 ? "mithu://downloads?q=" : page.indexOf("bookmarks") >= 0 ? "mithu://bookmarks?q=" : "";
  if (key) location.href = key + encodeURIComponent(q.value);
}});
function go(op, params, back) {{
  var url = "mithu://do?op=" + encodeURIComponent(op) + "&back=" + encodeURIComponent(back || location.href.split("?")[0]);
  Object.keys(params || {{}}).forEach(function (key) {{
    url += "&" + encodeURIComponent(key) + "=" + encodeURIComponent(params[key]);
  }});
  location.href = url;
}}
</script>
<p style="display:none">Mithu {version}</p>
</body></html>"#,
        fonts = roboto_faces(),
        theme = theme,
        font = prefs.font_px.clamp(12, 24),
        title = esc(title),
        icon = icon_src(),
        mark = mark_src(),
        layout = if matches!(title, "Downloads" | "New Tab" | "Tasks") { "solo" } else { "" },
        page_title = page_title(title),
        search_label = search_label(title),
        nav = nav_html(title, body, prefs),
        body = body,
        version = esc(version),
    )
}

fn page_title(title: &str) -> &'static str {
    match title {
        "Downloads" => "Download history",
        "Tasks" => "Task manager",
        "Extensions" => "Extensions",
        "History" => "History",
        "Bookmarks" => "Bookmarks",
        "New Tab" => "New tab",
        _ => "Settings",
    }
}

fn search_label(title: &str) -> &'static str {
    match title {
        "History" => "Search history",
        "Downloads" => "Search download history",
        "Bookmarks" => "Search bookmarks",
        "Extensions" => "Search extensions",
        "New Tab" => "Search",
        _ => "Search settings",
    }
}

fn nav_link(href: &str, label: &str, on: bool) -> String {
    format!(
        "<a href=\"{href}\"{class}>{icon}{label}</a>",
        class = if on { " class=\"on\"" } else { "" },
        icon = nav_icon(label),
        label = esc(label),
    )
}

fn nav_icon(label: &str) -> &'static str {
    match label {
        "You" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"8\" r=\"3\"/><path d=\"M5 19c1.5-3 4-4.5 7-4.5S17.5 16 19 19\"/></svg>",
        "Autofill and passwords" => "<svg viewBox=\"0 0 24 24\"><rect x=\"6\" y=\"10\" width=\"12\" height=\"9\" rx=\"1\"/><path d=\"M9 10V8a3 3 0 0 1 6 0v2\"/></svg>",
        "Privacy and security" => "<svg viewBox=\"0 0 24 24\"><path d=\"M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z\"/></svg>",
        "Performance" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><path d=\"M12 8v5l3 2\"/></svg>",
        "Appearance" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"3\"/><path d=\"M12 3v2M12 19v2M3 12h2M19 12h2\"/></svg>",
        "Search engine" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"11\" cy=\"11\" r=\"6\"/><path d=\"M16 16l4 4\"/></svg>",
        "Default browser" => "<svg viewBox=\"0 0 24 24\"><rect x=\"4\" y=\"6\" width=\"16\" height=\"12\" rx=\"1\"/></svg>",
        "On startup" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><path d=\"M12 8v4\"/></svg>",
        "Languages" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><path d=\"M4 12h16M12 4c2 3 2 13 0 16M12 4c-2 3-2 13 0 16\"/></svg>",
        "Downloads" => "<svg viewBox=\"0 0 24 24\"><path d=\"M12 4v10M8 10l4 4 4-4M5 19h14\"/></svg>",
        "Accessibility" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"5\" r=\"2\"/><path d=\"M5 9h14M12 9v11M8 20l4-6 4 6\"/></svg>",
        "System" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"3\"/><path d=\"M12 3v3M12 18v3M3 12h3M18 12h3\"/></svg>",
        "Reset settings" => "<svg viewBox=\"0 0 24 24\"><path d=\"M4 12a8 8 0 1 0 2-5\"/><path d=\"M4 4v5h5\"/></svg>",
        "Extensions" | "My extensions" => "<svg viewBox=\"0 0 24 24\"><path d=\"M8 8h3V4H8v4zm5 0h3V4h-3v4zM8 14h8v6H8z\"/></svg>",
        "About Mithu" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><path d=\"M12 11v5M12 8h.01\"/></svg>",
        "History" | "Mithu history" => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><path d=\"M12 8v5l3 2\"/></svg>",
        "Tabs from other devices" => "<svg viewBox=\"0 0 24 24\"><rect x=\"4\" y=\"6\" width=\"12\" height=\"10\" rx=\"1\"/><path d=\"M16 10h4v8H8\"/></svg>",
        "Delete browsing data" => "<svg viewBox=\"0 0 24 24\"><path d=\"M5 7h14M9 7V5h6v2M8 7l1 12h6l1-12\"/></svg>",
        "Keyboard shortcuts" => "<svg viewBox=\"0 0 24 24\"><rect x=\"3\" y=\"6\" width=\"18\" height=\"12\" rx=\"1\"/></svg>",
        "Bookmarks bar" | "Other bookmarks" => "<svg viewBox=\"0 0 24 24\"><path d=\"M4 6h7l2 2h7v10H4z\"/></svg>",
        _ => "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"3\"/></svg>",
    }
}

fn nav_html(title: &str, body: &str, prefs: &Prefs) -> String {
    if title == "Settings" || title == "Version" {
        let mut out = String::new();
        for (id, label) in SETTINGS_NAV {
            if *id == "about" {
                continue;
            }
            if *id == "languages" {
                out.push_str("<div class=\"sep\"></div>");
            }
            let selected = body.contains(&format!("id=\"{id}\" class=\"on\""));
            out.push_str(&nav_link(&format!("mithu://settings/{id}"), label, selected));
        }
        out.push_str("<div class=\"sep\"></div>");
        out.push_str(&nav_link("mithu://extensions", "Extensions", false));
        let about_on = body.contains("id=\"about\" class=\"on\"");
        out.push_str(&nav_link("mithu://settings/about", "About Mithu", about_on));
        return out;
    }
    if title == "History" {
        let synced = body.contains("data-view=\"synced\"");
        let mut out = nav_link("mithu://history", "Mithu history", !synced);
        out.push_str(&nav_link("mithu://history?view=synced", "Tabs from other devices", synced));
        out.push_str("<div class=\"sep\"></div>");
        out.push_str(&nav_link("mithu://settings/privacy", "Delete browsing data", false));
        return out;
    }
    if title == "Bookmarks" {
        let mut folders = vec!["Bookmarks bar".to_string(), "Other bookmarks".to_string()];
        for mark in &prefs.bookmarks {
            if !folders.iter().any(|folder| folder == &mark.folder) {
                folders.push(mark.folder.clone());
            }
        }
        let mut out = String::new();
        for folder in folders {
            let on = body.contains(&format!("data-folder=\"{}\"", esc(&folder)));
            out.push_str(&nav_link(
                &format!("mithu://bookmarks?folder={}", percent(&folder)),
                &folder,
                on,
            ));
        }
        return out;
    }
    if title == "Extensions" {
        let shortcuts = body.contains("data-view=\"shortcuts\"");
        let mut out = nav_link("mithu://extensions", "My extensions", !shortcuts);
        out.push_str(&nav_link("mithu://extensions?view=shortcuts", "Keyboard shortcuts", shortcuts));
        return out;
    }
    String::new()
}

const SETTINGS_NAV: &[(&str, &str)] = &[
    ("you", "You"),
    ("autofill", "Autofill and passwords"),
    ("privacy", "Privacy and security"),
    ("performance", "Performance"),
    ("appearance", "Appearance"),
    ("search", "Search engine"),
    ("default", "Default browser"),
    ("startup", "On startup"),
    ("languages", "Languages"),
    ("downloads", "Downloads"),
    ("accessibility", "Accessibility"),
    ("system", "System"),
    ("reset", "Reset settings"),
    ("about", "About Mithu"),
];

fn settings_body(spec: &str, prefs: &Prefs) -> String {
    let path = Url::parse(spec).map(|url| url.path().trim_matches('/').to_string()).unwrap_or_default();
    if path == "site" || path.starts_with("site/") {
        return site_settings_body(&path, prefs);
    }
    let raw = path.split('/').next().unwrap_or("");
    let focus = if raw.is_empty() { "you" } else { raw };
    let heading = SETTINGS_NAV.iter().find(|(id, _)| *id == focus).map(|(_, label)| *label).unwrap_or("Settings");
    let mut out = format!("<h1>{}</h1>", esc(heading));
    out.push_str(&section("you profile name", "You", &you_card(prefs), focus == "you"));
    out.push_str(&section("autofill passwords", "Autofill and passwords", "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Password manager<small>Nothing is stored until a lock is added. Nothing leaves this computer.</small></div></div><div class=\"row\"><div class=\"grow\">Payment methods</div></div><div class=\"row\"><div class=\"grow\">Addresses and more</div></div></div>", focus == "autofill"));
    out.push_str(&section("privacy security clear", "Privacy and security", &privacy_card(), focus == "privacy"));
    out.push_str(&section("performance memory", "Performance", "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Memory saver<small>Open tabs stay as they are. Mithu does not discard them in the background yet.</small></div></div></div>", focus == "performance"));
    out.push_str(&section("appearance theme font zoom home bookmarks", "Appearance", &appearance_card(prefs), focus == "appearance"));
    out.push_str(&section("search engine", "Search engine", &search_card(prefs), focus == "search"));
    out.push_str(&section("default browser", "Default browser", "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Mithu is not registered as the system browser.<small>The operating system still chooses which app opens links.</small></div></div></div>", focus == "default"));
    out.push_str(&section("startup on start", "On startup", &startup_card(prefs), focus == "startup"));
    out.push_str(&section("languages", "Languages", &lang_card(prefs), focus == "languages"));
    out.push_str(&section("downloads folder", "Downloads", &download_card(prefs), focus == "downloads"));
    out.push_str(&section("accessibility", "Accessibility", &a11y_card(prefs), focus == "accessibility"));
    out.push_str(&section("system hardware", "System", &system_card(prefs), focus == "system"));
    out.push_str(&section("reset", "Reset settings", "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Restore Mithu defaults<small>Bookmarks, history, and downloads stay.</small></div><button class=\"btn danger\" onclick=\"go('reset', {{}}, 'mithu://settings')\">Reset</button></div></div>", focus == "reset"));
    out.push_str(&section(
        "about version",
        "About Mithu",
        &format!(
            "<div class=\"card\"><div class=\"row\"><img class=\"markimg\" src=\"{mark}\" alt=\"\"><div class=\"grow\"><strong>Mithu</strong></div></div><div class=\"row\"><div class=\"grow\">Version<small>Also at mithu://version</small></div><a href=\"mithu://version\">Open</a></div><div class=\"row\"><div class=\"grow\">Open source notices<small>THIRD_PARTY_NOTICES.md</small></div></div></div>",
            mark = mark_src(),
        ),
        focus == "about",
    ));
    out
}

fn section(keys: &str, title: &str, body: &str, on: bool) -> String {
    format!(
        "<section data-section=\"{keys} {title}\" id=\"{id}\" class=\"{class}\"><h2>{title}</h2>{body}</section>",
        keys = esc(keys),
        title = esc(title),
        id = esc(keys.split_whitespace().next().unwrap_or("section")),
        class = if on { "on" } else { "hidden" },
        body = body,
    )
}

fn you_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><img class=\"markimg\" src=\"{mark}\" alt=\"\"><div class=\"grow\">{name}<small>Local profile. Not an account.</small></div></div>
<div class=\"row\"><div class=\"grow\">Customize profile</div><button class=\"btn quiet\" onclick=\"go('set', {{key:'name', value: prompt('Profile name', '{name}') || '{name}'}}, 'mithu://settings/you')\">Edit</button></div>
<div class=\"row\"><div class=\"grow\">Import bookmarks</div><a href=\"mithu://bookmarks\">Open</a></div></div>",
        name = esc(&prefs.name),
        mark = mark_src(),
    )
}

fn appearance_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\">
<div class=\"row\"><div class=\"grow\">Theme</div>
<select onchange=\"go('set', {{key:'theme', value:this.value}}, 'mithu://settings/appearance')\">{theme}</select></div>
<div class=\"row\"><div class=\"grow\">Font size<small>{font} px on Mithu pages</small></div>
<select onchange=\"go('set', {{key:'font_px', value:this.value}}, 'mithu://settings/appearance')\">{fonts}</select></div>
<div class=\"row\"><div class=\"grow\">Page zoom<small>{zoom}</small></div>
<select onchange=\"go('set', {{key:'zoom', value:this.value}}, 'mithu://settings/appearance')\">{zooms}</select></div>
<div class=\"row\"><div class=\"grow\">Show home button</div>{home}</div>
<div class=\"row\"><div class=\"grow\">Show bookmarks bar</div>{bar}</div>
</div>",
        theme = options(&[("light", "Light"), ("dark", "Dark")], &prefs.theme),
        font = prefs.font_px,
        fonts = options(&[("14", "14"), ("16", "16"), ("18", "18"), ("20", "20")], &prefs.font_px.to_string()),
        zoom = prefs.zoom,
        zooms = options(&[("0.8", "80%"), ("1", "100%"), ("1.25", "125%"), ("1.5", "150%"), ("2", "200%")], &format_zoom(prefs.zoom)),
        home = toggle("show_home", prefs.show_home, "mithu://settings/appearance"),
        bar = toggle("show_bookmarks", prefs.show_bookmarks, "mithu://settings/appearance"),
    )
}

fn search_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Address bar search<small>Used when the text is not a site name.</small></div>
<select onchange=\"go('set', {{key:'search', value:this.value}}, 'mithu://settings/search')\">{}</select></div></div>",
        options(&[("duckduckgo", "DuckDuckGo"), ("bing", "Bing"), ("wikipedia", "Wikipedia")], &prefs.search)
    )
}

fn startup_card(prefs: &Prefs) -> String {
    let pages = if prefs.startup_pages.is_empty() {
        "<small>No extra pages.</small>".into()
    } else {
        prefs.startup_pages.iter().map(|page| format!("<div>{}</div>", esc(page))).collect::<String>()
    };
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Open on launch</div>
<select onchange=\"go('set', {{key:'startup', value:this.value}}, 'mithu://settings/startup')\">{}</select></div>
<div class=\"row\"><div class=\"grow\">{pages}</div>
<button class=\"btn quiet\" onclick=\"go('startup-page', {{value: prompt('Page URL') || ''}}, 'mithu://settings/startup')\">Add page</button></div></div>",
        options(&[("newtab", "New tab"), ("continue", "Continue where you left off"), ("pages", "Specific pages")], &prefs.startup)
    )
}

fn lang_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Display language<small>{}</small></div>
<button class=\"btn quiet\" onclick=\"go('set', {{key:'languages', value: prompt('Languages', '{}') || '{}'}}, 'mithu://settings/languages')\">Edit</button></div></div>",
        esc(&prefs.languages),
        esc(&prefs.languages),
        esc(&prefs.languages),
    )
}

fn download_card(prefs: &Prefs) -> String {
    let dir = if prefs.download_dir.is_empty() { "System downloads folder".into() } else { esc(&prefs.download_dir) };
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Location<small>{dir}</small></div>
<button class=\"btn quiet\" onclick=\"go('set', {{key:'download_dir', value: prompt('Folder', '') || ''}}, 'mithu://settings/downloads')\">Change</button></div>
<div class=\"row\"><div class=\"grow\">Ask where to save each file</div>{}</div></div>",
        toggle("ask_where", prefs.ask_where, "mithu://settings/downloads")
    )
}

fn a11y_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Smooth scrolling</div>{}</div>
<div class=\"row\"><div class=\"grow\">Page text follows the font size in Appearance.</div></div></div>",
        toggle("smooth_scroll", prefs.smooth_scroll, "mithu://settings/accessibility")
    )
}

fn system_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Use hardware acceleration when available<small>The Servo build still falls back to software if the GPU context fails.</small></div>{}</div></div>",
        toggle("hardware", prefs.hardware, "mithu://settings/system")
    )
}

const SITE_PERMS: &[(&str, &str, &str)] = &[
    ("location", "Location", "permissions"),
    ("camera", "Camera", "permissions"),
    ("microphone", "Microphone", "permissions"),
    ("notifications", "Notifications", "permissions"),
    ("storage", "Third-party cookies and storage", "permissions"),
    ("background", "Background sync", "permissions"),
    ("sensors", "Motion sensors", "permissions"),
    ("downloads", "Automatic downloads", "permissions"),
    ("usb", "USB devices", "permissions"),
    ("clipboard", "Clipboard", "permissions"),
    ("cookies", "Cookies", "content"),
    ("javascript", "JavaScript", "content"),
    ("images", "Images", "content"),
    ("popups", "Pop-ups and redirects", "content"),
    ("sound", "Sound", "content"),
    ("pdf", "PDF documents", "content"),
];

fn site_known(id: &str) -> bool {
    SITE_PERMS.iter().any(|(key, _, _)| *key == id)
}

fn site_choice<'a>(prefs: &'a Prefs, id: &str) -> &'a str {
    prefs.site.iter().find(|(key, _)| key == id).map(|(_, value)| value.as_str()).unwrap_or("ask")
}

/// The choice the engine should enforce for one origin and one feature.
pub fn permission_for(prefs: &Prefs, origin: &str, feature: &str) -> String {
    if let Some(item) = prefs.exceptions.iter().find(|item| item.origin == origin && item.feature == feature) {
        return item.choice.clone();
    }
    site_choice(prefs, feature).to_string()
}

fn set_exception(prefs: &mut Prefs, value: &str) {
    let mut parts = value.splitn(3, ':');
    let origin = parts.next().unwrap_or("");
    let feature = parts.next().unwrap_or("");
    let choice = parts.next().unwrap_or("");
    if origin.is_empty() || !site_known(feature) || !matches!(choice, "ask" | "allow" | "block") {
        return;
    }
    prefs.exceptions.retain(|item| !(item.origin == origin && item.feature == feature));
    if choice != "ask" {
        prefs.exceptions.push(Exception {
            origin: origin.to_string(),
            feature: feature.to_string(),
            choice: choice.to_string(),
        });
    }
}

/// True when a main-frame navigation should be saved instead of shown.
pub fn is_download_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }
    let Some(name) = parsed.path_segments().and_then(|segments| segments.filter(|part| !part.is_empty()).last()) else {
        return false;
    };
    let ext = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("zip" | "7z" | "rar" | "gz" | "tgz" | "bz2" | "xz" | "dmg" | "exe" | "msi" | "apk" | "deb" | "rpm" | "iso" | "pkg" | "crx" | "bin" | "tar")
    )
}

fn site_settings_body(path: &str, prefs: &Prefs) -> String {
    let mut out = String::from("<section id=\"privacy\" class=\"on\" hidden></section>");
    if let Some(id) = path.strip_prefix("site/") {
        let Some((_, label, _)) = SITE_PERMS.iter().find(|(key, _, _)| *key == id) else {
            return format!("{out}<h1>Site settings</h1><p class=\"lead\">That permission is not in this build.</p>");
        };
        let current = site_choice(prefs, id);
        out.push_str(&format!(
            "<p class=\"lead\"><a href=\"mithu://settings/site\">Site settings</a></p><h1>{label}</h1><div class=\"card\">"
        ));
        for (value, name) in [("ask", "Ask first"), ("allow", "Allow"), ("block", "Block")] {
            let mark = if current == value { "On" } else { "Choose" };
            out.push_str(&format!(
                "<div class=\"row\"><div class=\"grow\">{name}</div><button class=\"btn quiet\" onclick=\"go('set', {{key:'perm', value:'{id}:{value}'}}, 'mithu://settings/site/{id}')\">{mark}</button></div>"
            ));
        }
        out.push_str("</div>");
        let mut listed = String::new();
        for item in prefs.exceptions.iter().filter(|item| item.feature == id) {
            listed.push_str(&format!(
                "<div class=\"row\"><div class=\"grow\">{}<small>{}</small></div><button class=\"btn quiet\" onclick=\"go('set', {{key:'exception', value:'{}:{id}:ask'}}, 'mithu://settings/site/{id}')\">Remove</button></div>",
                esc(&item.origin),
                esc(&item.choice),
                esc(&item.origin),
            ));
        }
        if listed.is_empty() {
            listed = "<div class=\"row\"><div class=\"grow\">No site exceptions<small>The engine uses the choice above until a site is allowed or blocked.</small></div></div>".into();
        }
        out.push_str(&format!("<p class=\"lead\">Exceptions</p><div class=\"card\">{listed}</div>"));
        return out;
    }
    out.push_str("<h1>Site settings</h1><p class=\"lead\">Permissions</p><div class=\"card\">");
    for (id, label, _) in SITE_PERMS.iter().filter(|(_, _, group)| *group == "permissions") {
        out.push_str(&site_row(id, label, site_choice(prefs, id)));
    }
    out.push_str("</div><p class=\"lead\">Content</p><div class=\"card\">");
    for (id, label, _) in SITE_PERMS.iter().filter(|(_, _, group)| *group == "content") {
        out.push_str(&site_row(id, label, site_choice(prefs, id)));
    }
    out.push_str("</div><p class=\"lead\"><a href=\"mithu://settings/privacy\">Privacy and security</a></p>");
    out
}

fn site_row(id: &str, label: &str, choice: &str) -> String {
    format!(
        "<div class=\"row\"><div class=\"grow\">{label}<small>{}</small></div><a href=\"mithu://settings/site/{id}\">Open</a></div>",
        match choice {
            "allow" => "Allowed",
            "block" => "Blocked",
            _ => "Ask first",
        }
    )
}

fn privacy_card() -> String {
    "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Delete browsing data<small>History stored by Mithu on this computer.</small></div>
<button class=\"btn danger\" onclick=\"go('forget-all', {}, 'mithu://settings/privacy')\">Delete data</button></div>
<div class=\"row\"><div class=\"grow\">Cookies and site data<small>Mithu does not keep a separate cookie list in this build.</small></div></div>
<div class=\"row\"><div class=\"grow\">Site settings<small>Location, camera, microphone, and the other permissions.</small></div><a href=\"mithu://settings/site\">Open</a></div>
<div class=\"row\"><div class=\"grow\">Security<small>HTTPS pages show a lock in the address bar.</small></div></div></div>".into()
}

fn history_body(spec: &str, prefs: &Prefs) -> String {
    let view = query_value(spec, "view");
    if view == "synced" {
        return "<div data-view=\"synced\"><h1>Tabs from other devices</h1><div class=\"empty\">No tabs from other devices</div></div>".into();
    }
    let q = query_value(spec, "q").to_ascii_lowercase();
    let grouped = query_value(spec, "group") == "host";
    let mut rows = String::new();
    if grouped {
        let mut hosts: Vec<String> = Vec::new();
        for visit in prefs.history.iter().rev() {
            if !q.is_empty() && !visit.url.to_ascii_lowercase().contains(&q) && !visit.title.to_ascii_lowercase().contains(&q) {
                continue;
            }
            let host = host_of(&visit.url);
            if !hosts.iter().any(|item| item == &host) {
                hosts.push(host);
            }
        }
        for host in hosts {
            rows.push_str(&format!("<h2>{}</h2><div class=\"card\">", esc(&host)));
            for visit in prefs.history.iter().rev() {
                if host_of(&visit.url) == host {
                    rows.push_str(&history_row(visit));
                }
            }
            rows.push_str("</div>");
        }
    } else {
        let mut last_day = String::new();
        for visit in prefs.history.iter().rev() {
            if !q.is_empty() && !visit.url.to_ascii_lowercase().contains(&q) && !visit.title.to_ascii_lowercase().contains(&q) {
                continue;
            }
            let day = day_heading(visit.at);
            if day != last_day {
                if !last_day.is_empty() {
                    rows.push_str("</div>");
                }
                rows.push_str(&format!("<h2 class=\"day\">{}</h2><div class=\"card\">", esc(&day)));
                last_day = day;
            }
            rows.push_str(&history_row(visit));
        }
        if !rows.is_empty() {
            rows.push_str("</div>");
        }
    }
    if rows.is_empty() {
        rows = empty_art("history", "Your browsing history appears here");
    }
    let date_on = if grouped { "" } else { " class=\"on\"" };
    let group_on = if grouped { " class=\"on\"" } else { "" };
    format!(
        "<div data-view=\"history\"><div class=\"tabs\"><a href=\"mithu://history\"{date_on}>By date</a><a href=\"mithu://history?group=host\"{group_on}>By group</a></div>{rows}</div>"
    )
}

fn history_row(visit: &Visit) -> String {
    format!(
        "<div class=\"hrow\"><div class=\"htime\">{}</div><div class=\"favicon\"></div><div class=\"hmain\"><a href=\"{}\">{}</a><small>{}</small></div><button class=\"btn quiet\" onclick=\"go('forget', {{value:'{}'}}, 'mithu://history')\">Delete</button></div>",
        esc(&clock_label(visit.at)),
        esc(&visit.url),
        esc(if visit.title.is_empty() { visit.url.as_str() } else { visit.title.as_str() }),
        esc(&host_of(&visit.url)),
        js(&visit.url),
    )
}

fn downloads_body(spec: &str, prefs: &Prefs) -> String {
    let q = query_value(spec, "q").to_ascii_lowercase();
    let mut rows = String::new();
    for item in prefs.downloads.iter().rev() {
        let name = std::path::Path::new(&item.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(item.path.as_str());
        if !q.is_empty() && !name.to_ascii_lowercase().contains(&q) && !item.url.to_ascii_lowercase().contains(&q) {
            continue;
        }
        rows.push_str(&format!(
            "<div class=\"dl\"><div class=\"fileico\"></div><div class=\"grow\"><a href=\"{}\">{}</a><small style=\"display:block;color:var(--muted)\">{}</small></div>
<button class=\"btn quiet\" onclick=\"go('download', {{key:'{}', value:'reveal'}}, 'mithu://downloads')\">Show in folder</button></div>",
            esc(&item.url),
            esc(name),
            esc(&item.state),
            js(&item.path),
        ));
    }
    if rows.is_empty() {
        return empty_art("downloads", "Files you download appear here");
    }
    format!("<div class=\"card\">{rows}</div>")
}

fn bookmarks_body(spec: &str, prefs: &Prefs) -> String {
    let export = export_html(prefs);
    let requested = query_value(spec, "folder");
    let selected = if requested.is_empty() { "Bookmarks bar".to_string() } else { requested };
    let q = query_value(spec, "q").to_ascii_lowercase();
    let mut list = String::new();
    let shown: Vec<_> = prefs
        .bookmarks
        .iter()
        .filter(|mark| mark.folder == selected)
        .filter(|mark| q.is_empty() || mark.title.to_ascii_lowercase().contains(&q) || mark.url.to_ascii_lowercase().contains(&q))
        .collect();
    if shown.is_empty() {
        list.push_str("<div class=\"empty\">To bookmark pages, click the star in the address bar</div>");
    } else {
        list.push_str("<div class=\"card\">");
        for mark in shown {
            list.push_str(&bookmark_row(mark));
        }
        list.push_str("</div>");
    }
    format!(
        "<div data-folder=\"{folder}\"><div class=\"row\" style=\"border:0\"><button class=\"btn quiet\" onclick=\"go('bookmark', {{value: prompt('URL')||'', title: prompt('Name')||'', folder: '{js_folder}'}}, 'mithu://bookmarks?folder={enc}')\">Add</button><button class=\"btn quiet\" onclick=\"go('import', {{value: prompt('Paste bookmark HTML')||''}}, 'mithu://bookmarks')\">Import</button><a class=\"btn quiet\" href=\"data:text/html,{export}\" download=\"mithu-bookmarks.html\">Export</a></div>{list}</div>",
        folder = esc(&selected),
        js_folder = js(&selected),
        enc = percent(&selected),
    )
}

fn translate_body() -> String {
    "<h1>Translate</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">Mithu does not translate pages.<small>The page stays on this computer. Nothing is sent to a translation service.</small></div><a class=\"btn quiet\" href=\"mithu://settings/languages\">Languages</a></div></div>".into()
}

fn empty_art(kind: &str, caption: &str) -> String {
    let svg = if kind == "downloads" {
        r##"<svg viewBox="0 0 120 100" width="120" height="100" aria-hidden="true"><path d="M38 28h28l10 10v40H38z" fill="#d3e3fd"/><path d="M66 28v10h10" fill="none" stroke="#0b57d0" stroke-width="3"/><path d="M58 70h22l-8 8v-14" fill="#0b57d0"/><path d="M70 36h16l8 8v28H70z" fill="#e8f0fe" stroke="#0b57d0" stroke-width="2"/><path d="M48 48h14v18H48z" fill="#8ab4f8"/></svg>"##
    } else {
        r##"<svg viewBox="48 48 104 104" width="104" height="104" aria-hidden="true"><path d="M89 55c6-5 15-5 21 0 1 .2 2 .5 2.5.8 2.7 2.1 6 3.4 9.5 3.5 7.3.4 13.4 5.5 15 12.6.1.3.2.7.4 1.4 2.7 4.1 5.4 6.3 8 7.5 6.4 4.9 7.8 12.8 4.4 19.3-.3.6-.6 1.1-1 1.7-1.6 3-2.3 6.6-1.8 10 .1.7.2 1.2.3 1.9 1 7.2-3.1 14.1-9.8 17-1.1.4-2 .8-3 1.2-3.2 1.3-6 3.6-7.8 6.5-.2.4-.5.8-.7 1.2-3.9 6.1-11.5 8.8-18.4 6.7-.3-.1-.6-.2-1.3-.4-3.3-1.1-6.9-1.1-10.2 0-.3.1-.6.2-1.3.4-6.9 2.1-14.5-.6-18.4-6.7-.2-.4-.4-.8-.7-1.2-1.8-2.9-4.6-5.2-7.8-6.5-1-.4-1.9-.8-3-1.2-6.7-2.9-10.7-9.8-9.8-17 .1-.7.2-1.2.3-1.9.5-3.4.0-7.1-1.8-10-.1-.2-.3-.5-1-1.7-3.4-6.5-2-14.4 4.4-19.3 2.6-1.2 5.3-3.4 8-7.5.1-.2.2-.5.4-1.4 1.6-7.1 7.7-12.2 15-12.6 3.5-.1 6.8-1.4 9.5-3.5.2-.1.5-.3.8-.5z" fill="#d3e3fd"/><path d="M88 71H79a8 8 0 0 0-8 8v9M113 71h9a8 8 0 0 1 8 8v9M88 130H79a8 8 0 0 1-8-8v-9M113 130h9a8 8 0 0 0 8-8v-9" fill="none" stroke="#0b57d0" stroke-width="6" stroke-linecap="round"/></svg>"##
    };
    format!("<div class=\"empty\">{svg}<p>{caption}</p></div>", caption = esc(caption))
}

thread_local! {
    static TASK_ROWS: std::cell::RefCell<Vec<(String, String, u8)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tabs shown on the task manager. The shell sets this before the page is built.
pub fn set_tasks(rows: Vec<(String, String, u8)>) {
    TASK_ROWS.with(|cell| *cell.borrow_mut() = rows);
}

fn task_body(_prefs: &Prefs) -> String {
    let rows = TASK_ROWS.with(|cell| cell.borrow().clone());
    if rows.is_empty() {
        return "<h1>Task manager</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">No tabs are open.</div></div></div>".into();
    }
    let mut out = String::from("<h1>Task manager</h1><div class=\"card\">");
    for (title, url, group) in rows {
        let group = if group == 0 { String::new() } else { format!("<small>Group {group}</small>") };
        out.push_str(&format!(
            "<div class=\"row\"><div class=\"grow\">{}{group}<small>{}</small></div></div>",
            esc(if title.is_empty() { "New tab" } else { &title }),
            esc(&url),
        ));
    }
    out.push_str("</div>");
    out
}

fn inspect_body() -> String {
    "<h1>Developer tools</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">Mithu Lite opens the system page inspector.<small>The Servo edition has no remote inspector in this build. Print uses the page's own print dialog when the engine provides one.</small></div></div></div>".into()
}

fn bookmark_row(mark: &Bookmark) -> String {
    format!(
        "<tr><td><a href=\"{}\">{}</a><small style=\"display:block;color:var(--muted)\">{}</small></td><td>
<button class=\"btn quiet\" onclick=\"go('bookmark', {{value:'{}', title: prompt('Name', '{}')||'{}', folder: prompt('Folder', '{}')||'{}'}}, 'mithu://bookmarks')\">Edit</button>
<button class=\"btn quiet\" onclick=\"go('bookmark-del', {{value:'{}'}}, 'mithu://bookmarks')\">Delete</button></td></tr>",
        esc(&mark.url),
        esc(&mark.title),
        esc(&mark.url),
        js(&mark.url),
        js(&mark.title),
        js(&mark.title),
        js(&mark.folder),
        js(&mark.folder),
        js(&mark.url),
    )
}

fn export_html(prefs: &Prefs) -> String {
    let mut html = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1><meta charset=utf-8><title>Bookmarks</title><h1>Bookmarks</h1><dl><p>");
    for mark in &prefs.bookmarks {
        html.push_str(&format!("<dt><a href=\"{}\">{}</a>", esc(&mark.url), esc(&mark.title)));
    }
    html.push_str("</dl><p>");
    percent(&html)
}

fn extensions_body(spec: &str) -> String {
    if query_value(spec, "view") == "shortcuts" {
        return "<div data-view=\"shortcuts\"><h1>Keyboard shortcuts</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">No extension shortcuts<small>Mithu does not load extension packages yet.</small></div></div></div></div>".into();
    }
    "<div data-view=\"extensions\"><h1>All extensions</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">No extensions installed<small>Mithu does not load extension packages yet.</small></div></div></div></div>".into()
}

fn newtab_body(prefs: &Prefs) -> String {
    let mut tiles = String::new();
    for mark in prefs.bookmarks.iter().take(8) {
        tiles.push_str(&format!(
            "<a class=\"tile\" href=\"{}\"><strong>{}</strong><small style=\"display:block;color:var(--muted)\">{}</small></a>",
            esc(&mark.url),
            esc(&mark.title),
            esc(&mark.folder)
        ));
    }
    if tiles.is_empty() {
        tiles = "<p class=\"lead\">Bookmark a page and it will show up here.</p>".into();
    }
    format!(
        "<div class=\"center\"><div><img class=\"logo\" src=\"{logo}\" alt=\"Mithu Browser\"><p class=\"lead\">Search from the address bar, or open a bookmark.</p><div class=\"tiles\">{tiles}</div></div></div>",
        logo = logo_src(),
    )
}

fn version_body(version: &str) -> String {
    format!(
        "<div id=\"about\" class=\"on\"><h1>About Mithu</h1><div class=\"card\"><div class=\"row\"><img class=\"markimg\" src=\"{mark}\" alt=\"\"><div class=\"grow\"><strong>Mithu</strong><small>Version {version}</small></div></div>
<div class=\"row\"><div class=\"grow\">Open source notices<small>THIRD_PARTY_NOTICES.md</small></div></div></div></div>",
        version = esc(version),
        mark = mark_src(),
    )
}

fn toggle(key: &str, on: bool, back: &str) -> String {
    let next = if on { "0" } else { "1" };
    let class = if on { "sw on" } else { "sw" };
    let label = if on { "On" } else { "Off" };
    format!(
        "<button class=\"{class}\" aria-pressed=\"{on}\" aria-label=\"{label}\" onclick=\"go('set', {{key:'{key}', value:'{next}'}}, '{back}')\"><i></i></button>"
    )
}

fn options(choices: &[(&str, &str)], current: &str) -> String {
    let mut out = String::new();
    for (value, label) in choices {
        let selected = if *value == current { " selected" } else { "" };
        out.push_str(&format!("<option value=\"{value}\"{selected}>{label}</option>"));
    }
    out
}

fn format_zoom(zoom: f32) -> String {
    if (zoom - 0.8).abs() < 0.01 {
        "0.8".into()
    } else if (zoom - 1.25).abs() < 0.01 {
        "1.25".into()
    } else if (zoom - 1.5).abs() < 0.01 {
        "1.5".into()
    } else if (zoom - 2.0).abs() < 0.01 {
        "2".into()
    } else {
        "1".into()
    }
}

fn query_value(spec: &str, key: &str) -> String {
    let Some(query) = spec.split_once('?').map(|(_, q)| q) else {
        return String::new();
    };
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return decode(v);
            }
        }
    }
    String::new()
}

fn clock_label(at: u64) -> String {
    if at == 0 {
        return String::new();
    }
    let secs = at % 86_400;
    format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

fn host_of(url: &str) -> String {
    Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string())
}

fn day_heading(at: u64) -> String {
    let iso = day_label(at);
    let Some((year, rest)) = iso.split_once('-') else {
        return iso;
    };
    let Some((month, day)) = rest.split_once('-') else {
        return iso;
    };
    let name = match month {
        "01" => "January",
        "02" => "February",
        "03" => "March",
        "04" => "April",
        "05" => "May",
        "06" => "June",
        "07" => "July",
        "08" => "August",
        "09" => "September",
        "10" => "October",
        "11" => "November",
        "12" => "December",
        _ => return iso,
    };
    let day_n: u32 = day.parse().unwrap_or(0);
    format!("{day_n} {name} {year}")
}

fn day_label(at: u64) -> String {
    if at == 0 {
        return "Earlier".into();
    }
    // Howard Hinnant's civil_from_days, UTC. 1_700_000_000 is 2023-11-14.
    let z = (at / 86_400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn js(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "\\'")
}

fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn format_prefs(prefs: &Prefs) -> String {
    let mut out = String::new();
    push_line(&mut out, "theme", &prefs.theme);
    push_line(&mut out, "font_px", &prefs.font_px.to_string());
    push_line(&mut out, "zoom", &prefs.zoom.to_string());
    push_line(&mut out, "show_home", if prefs.show_home { "1" } else { "0" });
    push_line(&mut out, "show_bookmarks", if prefs.show_bookmarks { "1" } else { "0" });
    push_line(&mut out, "search", &prefs.search);
    push_line(&mut out, "startup", &prefs.startup);
    push_line(&mut out, "download_dir", &prefs.download_dir);
    push_line(&mut out, "languages", &prefs.languages);
    push_line(&mut out, "ask_where", if prefs.ask_where { "1" } else { "0" });
    push_line(&mut out, "smooth_scroll", if prefs.smooth_scroll { "1" } else { "0" });
    push_line(&mut out, "hardware", if prefs.hardware { "1" } else { "0" });
    push_line(&mut out, "name", &prefs.name);
    for page in &prefs.startup_pages {
        push_line(&mut out, "startup_page", page);
    }
    for mark in &prefs.bookmarks {
        out.push_str(&format!("bookmark\t{}\t{}\t{}\n", mark.folder, mark.title, mark.url));
    }
    for visit in &prefs.history {
        out.push_str(&format!("history\t{}\t{}\t{}\n", visit.at, visit.title, visit.url));
    }
    for item in &prefs.downloads {
        out.push_str(&format!("download\t{}\t{}\t{}\n", item.state, item.url, item.path));
    }
    for (id, choice) in &prefs.site {
        out.push_str(&format!("site\t{id}\t{choice}\n"));
    }
    for item in &prefs.exceptions {
        out.push_str(&format!("exception\t{}\t{}\t{}\n", item.origin, item.feature, item.choice));
    }
    out
}

fn push_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push_str(&value.replace('\n', " "));
    out.push('\n');
}

fn parse_prefs(text: &str) -> Prefs {
    let mut prefs = Prefs::default();
    prefs.startup_pages.clear();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("bookmark\t") {
            let mut parts = rest.splitn(3, '\t');
            let folder = parts.next().unwrap_or("").to_string();
            let title = parts.next().unwrap_or("").to_string();
            let url = parts.next().unwrap_or("").to_string();
            if !url.is_empty() {
                prefs.bookmarks.push(Bookmark { title, url, folder });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("history\t") {
            let mut parts = rest.splitn(3, '\t');
            let at = parts.next().unwrap_or("0").parse().unwrap_or(0);
            let title = parts.next().unwrap_or("").to_string();
            let url = parts.next().unwrap_or("").to_string();
            if !url.is_empty() {
                prefs.history.push(Visit { title, url, at });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("download\t") {
            let mut parts = rest.splitn(3, '\t');
            let state = parts.next().unwrap_or("done").to_string();
            let url = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            prefs.downloads.push(Download { url, path, state });
            continue;
        }
        if let Some(rest) = line.strip_prefix("exception\t") {
            let mut parts = rest.splitn(3, '\t');
            let origin = parts.next().unwrap_or("").to_string();
            let feature = parts.next().unwrap_or("").to_string();
            let choice = parts.next().unwrap_or("ask").to_string();
            if !origin.is_empty() && site_known(&feature) && matches!(choice.as_str(), "allow" | "block") {
                prefs.exceptions.push(Exception { origin, feature, choice });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("site\t") {
            let mut parts = rest.splitn(2, '\t');
            let id = parts.next().unwrap_or("").to_string();
            let choice = parts.next().unwrap_or("ask").to_string();
            if site_known(&id) && matches!(choice.as_str(), "ask" | "allow" | "block") {
                prefs.site.push((id, choice));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "theme" => prefs.theme = value.into(),
            "font_px" => prefs.font_px = value.parse().unwrap_or(16),
            "zoom" => prefs.zoom = value.parse().unwrap_or(1.0),
            "show_home" => prefs.show_home = value != "0",
            "show_bookmarks" => prefs.show_bookmarks = value != "0",
            "search" => prefs.search = value.into(),
            "startup" => prefs.startup = value.into(),
            "startup_page" => prefs.startup_pages.push(value.into()),
            "download_dir" => prefs.download_dir = value.into(),
            "languages" => prefs.languages = value.into(),
            "ask_where" => prefs.ask_where = value != "0",
            "smooth_scroll" => prefs.smooth_scroll = value != "0",
            "hardware" => prefs.hardware = value != "0",
            "name" => prefs.name = value.into(),
            _ => {}
        }
    }
    prefs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_round_trip_and_skip_other_brands() {
        let mut prefs = Prefs::default();
        prefs.bookmarks.push(Bookmark { title: "Notes".into(), url: "https://example.com".into(), folder: "Bar".into() });
        prefs.remember("https://example.com/a", "A");
        let text = format_prefs(&prefs);
        let loaded = parse_prefs(&text);
        assert_eq!(loaded.bookmarks.len(), 1);
        assert_eq!(loaded.history.len(), 1);
        let doc = open("mithu://settings/appearance", &loaded, "0.1.3").unwrap();
        let visible = strip_data_uris(&doc.html).to_ascii_lowercase();
        assert!(!visible.contains("google"));
        assert!(!visible.contains("chrome"));
        assert!(doc.html.contains("Appearance"));
        assert!(doc.html.contains("class=\"on\""));
        let translate = open("mithu://translate", &loaded, "0.1.5").unwrap();
        let visible = strip_data_uris(&translate.html).to_ascii_lowercase();
        assert!(visible.contains("does not translate"));
        assert!(!visible.contains("google"));
        assert!(open("mithu://inspect", &loaded, "0.1.5").unwrap().html.contains("Developer tools"));
        assert!(open("mithu://settings", &loaded, "0.1.5").unwrap().html.contains("id=\"you\" class=\"on\""));
        assert!(open("mithu://version", &loaded, "9.9.9").unwrap().html.contains("9.9.9"));
        assert!(open("about:history", &loaded, "0.1.3").unwrap().html.contains("example.com/a"));
        assert_eq!(day_label(1_700_000_000), "2023-11-14");
        assert_eq!(day_label(0), "Earlier");
        command("mithu://do?op=set&key=theme&value=dark&back=mithu://settings", &mut prefs).unwrap();
        assert!(prefs.dark());
        let search = search_url("wikipedia", "mithu");
        assert_eq!(search.domain(), Some("en.wikipedia.org"));
        assert!(is_internal("mithu://extensions"));
        assert!(is_internal("mithu://tasks"));
        assert!(!is_internal("https://example.com"));
        command("mithu://do?op=set&key=perm&value=camera:block&back=mithu://settings/site/camera", &mut prefs).unwrap();
        let mut stored = parse_prefs(&format_prefs(&prefs));
        assert_eq!(stored.site, vec![("camera".into(), "block".into())]);
        let site = open("mithu://settings/site/camera", &stored, "0.1.6").unwrap();
        assert!(site.html.contains("Block"));
        assert!(!strip_data_uris(&site.html).to_ascii_lowercase().contains("chrome"));
        set_tasks(vec![("Notes".into(), "https://example.com".into(), 1)]);
        assert!(open("mithu://tasks", &stored, "0.1.6").unwrap().html.contains("Notes"));
        assert_eq!(download_name("https://example.com/a/page.html?q=1"), "page.html");
        set_exception(&mut stored, "example.com:camera:allow");
        assert_eq!(permission_for(&stored, "example.com", "camera"), "allow");
        assert_eq!(permission_for(&stored, "other.test", "camera"), "block");
        assert!(is_download_url("https://example.com/files/mithu.zip"));
        assert!(!is_download_url("https://example.com/watch"));
    }

    #[test]
    fn write_page_fixtures() {
        let Ok(dir) = std::env::var("MITHU_PAGES_DIR") else {
            return;
        };
        let mut prefs = Prefs::default();
        prefs.bookmarks.push(Bookmark {
            title: "Notes".into(),
            url: "https://example.com/notes".into(),
            folder: "Bookmarks bar".into(),
        });
        prefs.history.push(Visit {
            title: "Example".into(),
            url: "https://example.com".into(),
            at: 1_700_000_000,
        });
        prefs.downloads.push(Download {
            url: "https://example.com/file.txt".into(),
            path: "/tmp/file.txt".into(),
            state: "done".into(),
        });
        set_tasks(vec![("Notes".into(), "https://example.com".into(), 1)]);
        for (theme, suffix) in [("light", "light"), ("dark", "dark")] {
            prefs.theme = theme.into();
            for spec in [
                "mithu://settings",
                "mithu://settings/privacy",
                "mithu://settings/site",
                "mithu://settings/site/camera",
                "mithu://history",
                "mithu://downloads",
                "mithu://bookmarks",
                "mithu://extensions",
                "mithu://newtab",
                "mithu://version",
                "mithu://tasks",
            ] {
                let doc = open(spec, &prefs, "0.1.3").expect(spec);
                let name = spec.trim_start_matches("mithu://");
                let path = std::path::Path::new(&dir).join(format!("{name}-{suffix}.html"));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("page dir");
                }
                std::fs::write(path, doc.html).expect("write page");
            }
            let history = prefs.history.clone();
            let downloads = prefs.downloads.clone();
            prefs.history.clear();
            prefs.downloads.clear();
            set_tasks(Vec::new());
            for spec in ["mithu://history", "mithu://downloads", "mithu://tasks"] {
                let doc = open(spec, &prefs, "0.1.6").expect(spec);
                let name = spec.trim_start_matches("mithu://");
                let path = std::path::Path::new(&dir).join(format!("empty-{name}-{suffix}.html"));
                std::fs::write(path, doc.html).expect("write empty page");
            }
            prefs.history = history;
            prefs.downloads = downloads;
            set_tasks(vec![("Notes".into(), "https://example.com".into(), 1)]);
        }
    }

    #[test]
    fn fractional_scales_are_not_this_module() {
        let _ = Prefs::default();
    }
}
