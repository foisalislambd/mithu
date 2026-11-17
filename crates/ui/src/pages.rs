//! `mithu://` pages shared by Mithu and Mithu Lite.
//!
//! The documents are generated here so both editions show the same settings,
//! history, downloads, bookmarks, and new tab. Nothing in the markup names
//! another browser or uses its marks.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("Mithu");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("Mithu")
}

pub fn path() -> PathBuf {
    directory().join("prefs.mithu")
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
    while let Some(start) = rest.find("data:image") {
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
        _ => None,
    }
}

fn render(kind: Kind, spec: &str, prefs: &Prefs, version: &str) -> Document {
    let (url, title, body) = match kind {
        Kind::Settings => ("mithu://settings", "Settings", settings_body(spec, prefs)),
        Kind::History => ("mithu://history", "History", history_body(spec, prefs)),
        Kind::Downloads => ("mithu://downloads", "Downloads", downloads_body(prefs)),
        Kind::Bookmarks => ("mithu://bookmarks", "Bookmarks", bookmarks_body(prefs)),
        Kind::Extensions => ("mithu://extensions", "Extensions", extensions_body()),
        Kind::NewTab => ("mithu://newtab", "New Tab", newtab_body(prefs)),
        Kind::Version => ("mithu://version", "Version", version_body(version)),
    };
    Document {
        url: url.into(),
        title: title.into(),
        html: shell(title, prefs, version, &body),
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
:root {{ color-scheme: light; --bg:#f4f1ea; --card:#fff; --ink:#1c1b19; --muted:#5f5a54; --line:#e4ddd2; --field:#f7f4ee; --accent:#0f766e; --accent-ink:#fff; --side:#efeae2; --hover:#e7f3f1; --danger:#9f1239; }}
html[data-theme="dark"] {{ color-scheme: dark; --bg:#14161a; --card:#1e2128; --ink:#e7e4de; --muted:#b0aaa2; --line:#343841; --field:#171a20; --accent:#5eead4; --accent-ink:#08332f; --side:#191c22; --hover:#243330; --danger:#fda4af; }}
* {{ box-sizing: border-box; }}
html, body {{ margin:0; background:var(--bg); color:var(--ink); font: {font}px/1.45 "Noto Sans", sans-serif; }}
a {{ color: var(--accent); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
button, input, select, textarea {{ font: inherit; color: inherit; }}
.top {{ display:flex; align-items:center; gap:16px; height:56px; padding:0 20px; background:var(--card); border-bottom:1px solid var(--line); position:sticky; top:0; }}
.markimg {{ width:28px; height:28px; }}
img.logo {{ width:min(280px, 72vw); height:auto; display:block; margin:0 auto 8px; }}
.brand {{ font-weight:650; letter-spacing:-0.02em; }}
.search {{ margin-left:auto; width:min(360px, 46vw); background:var(--field); border:1px solid var(--line); border-radius:999px; padding:8px 14px; }}
.layout {{ display:grid; grid-template-columns: 232px 1fr; min-height: calc(100vh - 56px); }}
nav {{ background:var(--side); padding:12px; border-right:1px solid var(--line); }}
nav a {{ display:block; padding:8px 12px; border-radius:8px; color:var(--ink); margin:2px 0; }}
nav a:hover, nav a.on {{ background:var(--hover); text-decoration:none; }}
main {{ padding:28px 32px 64px; max-width:860px; }}
h1 {{ font-size:28px; line-height:1.2; margin:0 0 8px; letter-spacing:-0.03em; }}
h2 {{ font-size:18px; margin:28px 0 8px; }}
.lead {{ color:var(--muted); margin:0 0 20px; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:4px 16px; margin:12px 0 20px; }}
.row {{ display:flex; align-items:center; gap:16px; min-height:52px; border-top:1px solid var(--line); padding:8px 0; }}
.card .row:first-child {{ border-top:0; }}
.row .grow {{ flex:1; }}
.row small {{ display:block; color:var(--muted); }}
select, .text {{ background:var(--field); border:1px solid var(--line); border-radius:8px; padding:8px 10px; }}
.btn {{ border:0; background:var(--accent); color:var(--accent-ink); border-radius:999px; padding:8px 14px; cursor:pointer; }}
.btn.quiet {{ background:transparent; color:var(--ink); border:1px solid var(--line); }}
.btn.danger {{ background:transparent; color:var(--danger); border:1px solid var(--danger); }}
.tiles {{ display:grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap:12px; }}
.tile {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:16px; min-height:88px; }}
.center {{ min-height:70vh; display:grid; place-items:center; text-align:center; }}
.hero {{ width:72px; height:72px; border-radius:20px; background:var(--accent); color:var(--accent-ink); display:grid; place-items:center; font-size:32px; font-weight:700; margin:0 auto 12px; }}
table {{ width:100%; border-collapse:collapse; }}
td {{ padding:10px 8px; border-top:1px solid var(--line); vertical-align:top; }}
.hidden {{ display:none !important; }}
</style>
</head>
<body>
<header class="top"><img class="markimg" src="{mark}" alt=""><div class="brand">Mithu</div>
<input class="search" id="q" placeholder="Search settings" aria-label="Search settings"></header>
<div class="layout">
<nav>
<a href="mithu://newtab">New tab</a>
<a href="mithu://settings">Settings</a>
<a href="mithu://history">History</a>
<a href="mithu://downloads">Downloads</a>
<a href="mithu://bookmarks">Bookmarks</a>
<a href="mithu://extensions">Extensions</a>
<a href="mithu://version">About</a>
</nav>
<main>{body}</main>
</div>
<script>
var q = document.getElementById("q");
q.addEventListener("input", function () {{
  var needle = q.value.toLowerCase();
  document.querySelectorAll("[data-section]").forEach(function (node) {{
    var hay = (node.getAttribute("data-section") || "").toLowerCase();
    node.classList.toggle("hidden", needle && hay.indexOf(needle) < 0);
  }});
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
        theme = theme,
        font = prefs.font_px.clamp(12, 24),
        title = esc(title),
        icon = crate::brand::favicon_data_uri(),
        mark = crate::brand::mark_data_uri(),
        body = body,
        version = esc(version),
    )
}

fn settings_body(spec: &str, prefs: &Prefs) -> String {
    let focus = spec.split('/').nth(3).unwrap_or("").split('?').next().unwrap_or("");
    let mut out = String::from("<h1>Settings</h1><p class=\"lead\">Mithu stores these on this computer and applies them in both editions.</p>");
    out.push_str(&section("you profile name", "You", &you_card(prefs), focus == "you"));
    out.push_str(&section("autofill passwords", "Autofill and passwords", "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Passwords and cards stay empty until a lock is added. Nothing is sent off this computer.</div></div></div>", focus == "autofill"));
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
            "<img class=\"logo\" src=\"{}\" alt=\"Mithu Browser\"><div class=\"card\"><div class=\"row\"><div class=\"grow\">Version<small>Also at mithu://version</small></div><a href=\"mithu://version\">Open</a></div></div>",
            crate::brand::logo_data_uri()
        ),
        focus == "about",
    ));
    out
}

fn section(keys: &str, title: &str, body: &str, on: bool) -> String {
    format!(
        "<section data-section=\"{keys} {title}\" id=\"{id}\"><h2>{title}</h2>{body}</section>",
        keys = esc(keys),
        title = esc(title),
        id = esc(keys.split_whitespace().next().unwrap_or("section")),
        body = body,
        // on is reserved for a future scroll; the id is enough
    ) + if on { "" } else { "" }
}

fn you_card(prefs: &Prefs) -> String {
    format!(
        "<div class=\"card\"><div class=\"row\"><img class=\"markimg\" src=\"{mark}\" alt=\"\"><div class=\"grow\">{name}<small>Local profile. Not an account.</small></div>
<button class=\"btn quiet\" onclick=\"go('set', {{key:'name', value: prompt('Profile name', '{name}') || '{name}'}}, 'mithu://settings/you')\">Edit</button></div></div>",
        name = esc(&prefs.name),
        mark = crate::brand::mark_data_uri(),
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

fn privacy_card() -> String {
    "<div class=\"card\"><div class=\"row\"><div class=\"grow\">Clear history<small>Pages you visited in Mithu on this computer.</small></div>
<button class=\"btn danger\" onclick=\"go('forget-all', {}, 'mithu://history')\">Clear</button></div></div>".into()
}

fn history_body(spec: &str, prefs: &Prefs) -> String {
    let q = query_value(spec, "q").to_ascii_lowercase();
    let mut rows = String::new();
    let mut last_day = String::new();
    for visit in prefs.history.iter().rev() {
        if !q.is_empty() && !visit.url.to_ascii_lowercase().contains(&q) && !visit.title.to_ascii_lowercase().contains(&q) {
            continue;
        }
        let day = day_label(visit.at);
        if day != last_day {
            rows.push_str(&format!("<h2>{}</h2><div class=\"card\"><table>", esc(&day)));
            last_day = day;
        }
        rows.push_str(&format!(
            "<tr><td><a href=\"{}\">{}</a><small style=\"display:block;color:var(--muted)\">{}</small></td><td><button class=\"btn quiet\" onclick=\"go('forget', {{value:'{}'}}, 'mithu://history')\">Delete</button></td></tr>",
            esc(&visit.url),
            esc(&visit.title),
            esc(&visit.url),
            js(&visit.url),
        ));
    }
    if !rows.is_empty() {
        rows.push_str("</table></div>");
    }
    if rows.is_empty() {
        rows = "<p class=\"lead\">No matching history.</p>".into();
    }
    format!(
        "<h1>History</h1><div class=\"card\"><div class=\"row\"><input class=\"text grow\" placeholder=\"Search history\" value=\"{}\" onchange=\"location.href='mithu://history?q='+encodeURIComponent(this.value)\"></div></div>{rows}",
        esc(&q)
    )
}

fn downloads_body(prefs: &Prefs) -> String {
    if prefs.downloads.is_empty() {
        return "<h1>Downloads</h1><p class=\"lead\">Files you save show up here.</p>".into();
    }
    let mut rows = String::from("<div class=\"card\"><table>");
    for item in prefs.downloads.iter().rev() {
        rows.push_str(&format!(
            "<tr><td>{}<small style=\"display:block;color:var(--muted)\">{} · {}</small></td><td>
<button class=\"btn quiet\" onclick=\"go('download', {{key:'{}', value:'open'}}, 'mithu://downloads')\">Open</button>
<button class=\"btn quiet\" onclick=\"go('download', {{key:'{}', value:'reveal'}}, 'mithu://downloads')\">Show in folder</button>
<button class=\"btn quiet\" onclick=\"go('download', {{key:'{}', value:'cancel'}}, 'mithu://downloads')\">Cancel</button>
<button class=\"btn quiet\" onclick=\"go('download', {{key:'{}', value:'retry'}}, 'mithu://downloads')\">Retry</button>
</td></tr>",
            esc(&item.path),
            esc(&item.url),
            esc(&item.state),
            js(&item.path),
            js(&item.path),
            js(&item.path),
            js(&item.path),
        ));
    }
    rows.push_str("</table></div>");
    format!("<h1>Downloads</h1>{rows}")
}

fn bookmarks_body(prefs: &Prefs) -> String {
    let export = export_html(prefs);
    let mut groups: Vec<(String, String)> = Vec::new();
    for mark in &prefs.bookmarks {
        if let Some((_, html)) = groups.iter_mut().find(|(folder, _)| folder == &mark.folder) {
            html.push_str(&bookmark_row(mark));
        } else {
            groups.push((mark.folder.clone(), bookmark_row(mark)));
        }
    }
    let mut body = String::from("<h1>Bookmarks</h1><p class=\"lead\">Folders live in the Mithu profile.</p>");
    body.push_str("<div class=\"card\"><div class=\"row\"><button class=\"btn\" onclick=\"go('bookmark', {value: prompt('URL')||'', title: prompt('Name')||'', folder: prompt('Folder', 'Bookmarks bar')||'Bookmarks bar'}, 'mithu://bookmarks')\">Add</button>");
    body.push_str("<button class=\"btn quiet\" onclick=\"go('import', {value: prompt('Paste bookmark HTML')||''}, 'mithu://bookmarks')\">Import HTML</button>");
    body.push_str(&format!("<a class=\"btn quiet\" href=\"data:text/html,{export}\" download=\"mithu-bookmarks.html\">Export HTML</a></div></div>"));
    if groups.is_empty() {
        body.push_str("<p class=\"lead\">No bookmarks yet.</p>");
    }
    for (folder, rows) in groups {
        body.push_str(&format!("<h2>{}</h2><div class=\"card\"><table>{rows}</table></div>", esc(&folder)));
    }
    body
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

fn extensions_body() -> String {
    "<h1>Extensions</h1><div class=\"card\"><div class=\"row\"><div class=\"grow\">No extensions installed.<small>Mithu does not load third-party extension packages yet. This page is the place they will be managed.</small></div></div></div>".into()
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
        logo = crate::brand::logo_data_uri(),
    )
}

fn version_body(version: &str) -> String {
    format!(
        "<h1>About Mithu</h1><img class=\"logo\" src=\"{logo}\" alt=\"Mithu Browser\"><div class=\"card\"><div class=\"row\"><img class=\"markimg\" src=\"{mark}\" alt=\"\"><div class=\"grow\">Mithu<small>Version {version}</small></div></div>
<div class=\"row\"><div class=\"grow\">Revision source<small>The first line of VERSION.md, copied into the binary.</small></div></div>
<div class=\"row\"><div class=\"grow\">Updates<small>A newer tag is published from that same line.</small></div></div></div>",
        version = esc(version),
        logo = crate::brand::logo_data_uri(),
        mark = crate::brand::mark_data_uri(),
    )
}

fn toggle(key: &str, on: bool, back: &str) -> String {
    let next = if on { "0" } else { "1" };
    let label = if on { "On" } else { "Off" };
    format!(
        "<button class=\"btn quiet\" onclick=\"go('set', {{key:'{key}', value:'{next}'}}, '{back}')\">{label}</button>"
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

fn day_label(at: u64) -> String {
    if at == 0 {
        return "Earlier".into();
    }
    let days = at / 86_400;
    format!("Day {days}")
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
        assert!(open("mithu://version", &loaded, "9.9.9").unwrap().html.contains("9.9.9"));
        assert!(open("about:history", &loaded, "0.1.3").unwrap().html.contains("example.com/a"));
        command("mithu://do?op=set&key=theme&value=dark&back=mithu://settings", &mut prefs).unwrap();
        assert!(prefs.dark());
        let search = search_url("wikipedia", "mithu");
        assert_eq!(search.domain(), Some("en.wikipedia.org"));
        assert!(is_internal("mithu://extensions"));
        assert!(!is_internal("https://example.com"));
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
        for (theme, suffix) in [("light", "light"), ("dark", "dark")] {
            prefs.theme = theme.into();
            for spec in [
                "mithu://settings",
                "mithu://history",
                "mithu://downloads",
                "mithu://bookmarks",
                "mithu://extensions",
                "mithu://newtab",
                "mithu://version",
            ] {
                let doc = open(spec, &prefs, "0.1.3").expect(spec);
                let name = spec.trim_start_matches("mithu://");
                let path = std::path::Path::new(&dir).join(format!("{name}-{suffix}.html"));
                std::fs::write(path, doc.html).expect("write page");
            }
        }
    }

    #[test]
    fn fractional_scales_are_not_this_module() {
        let _ = Prefs::default();
    }
}
