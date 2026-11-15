//! Blocking HTTP/1.1 client with TLS, cookies, and a small response cache.
//!
//! Mithu speaks HTTP itself: request formatting, content-length, chunked
//! bodies, redirects, and `Cookie` / `Set-Cookie`. TLS is provided by Rustls
//! so the certificate check is a real one, not a hand-rolled cipher. One
//! profile-wide jar and cache stand in for Chromium's network service.
//! HTTP/3 is not spoken: that needs a QUIC stack, and the client only
//! advertises `h2` and `http/1.1`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mithu_url::Url;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

mod cache;
mod cookies;
mod http2;

pub use cache::HttpCache;
pub use cookies::{Cookie, CookieJar};

/// Chrome's current stable major, with Mithu named after it the way Edge and Brave do.
pub const CHROME_MAJOR: &str = "157";
pub const PRODUCT_VERSION: &str = "0.1.0";
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Mithu/0.1.0";
pub const SEC_CH_UA: &str = "\"Chromium\";v=\"157\", \"Mithu\";v=\"0\", \"Not)A;Brand\";v=\"24\"";
pub const SEC_CH_UA_MOBILE: &str = "?0";
pub const SEC_CH_UA_PLATFORM: &str = "\"Linux\"";

/// Non-`HttpOnly` cookies visible to `document.cookie` on `url`.
pub fn document_cookie(url: &Url) -> String {
    with_profile(|profile| profile.cookies.document_cookie(url))
}

/// Stores a `document.cookie` assignment. `HttpOnly` cannot be set from script.
pub fn set_document_cookie(url: &Url, line: &str) {
    with_profile(|profile| profile.cookies.set_document_cookie(url, line));
}

/// Stores one `Set-Cookie` header, including `HttpOnly`, the way a response does.
pub fn store_set_cookie(url: &Url, header: &str) {
    with_profile(|profile| {
        profile
            .cookies
            .ingest(url, &[("set-cookie".to_string(), header.to_string())]);
    });
}

/// How a navigation uses the profile cache. Reload bypasses it; other loads may reuse a fresh response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheMode {
    Use,
    Bypass,
}

struct Profile {
    cookies: CookieJar,
    cache: HttpCache,
}

fn profile() -> &'static Mutex<Profile> {
    static PROFILE: OnceLock<Mutex<Profile>> = OnceLock::new();
    PROFILE.get_or_init(|| {
        Mutex::new(Profile {
            cookies: CookieJar::new(),
            cache: HttpCache::new(32),
        })
    })
}

fn with_profile<T>(body: impl FnOnce(&mut Profile) -> T) -> T {
    let mut guard = profile()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    body(&mut guard)
}

#[derive(Debug)]
pub enum NetError {
    Io(String),
    Tls(String),
    BadResponse,
    EmptyResponse,
    Timeout,
    TooLarge,
    TooManyRedirects,
    InvalidRedirect,
    InvalidUrl,
    UnsupportedScheme(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Tls(message) => write!(f, "{message}"),
            Self::BadResponse => write!(f, "the server sent an incomplete HTTP response"),
            Self::EmptyResponse => write!(f, "the server closed the connection without data"),
            Self::Timeout => write!(f, "the connection timed out"),
            Self::TooLarge => write!(f, "the response is larger than 10 MB"),
            Self::TooManyRedirects => write!(f, "too many redirects"),
            Self::InvalidRedirect => write!(f, "invalid redirect target"),
            Self::InvalidUrl => write!(f, "the url has no host"),
            Self::UnsupportedScheme(scheme) => write!(f, "cannot fetch {scheme} urls"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<std::io::Error> for NetError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub url: Url,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| key == &name)
            .map(|(_, value)| value.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
            .map(|value| value.split(';').next().unwrap_or(value).trim())
    }

    pub fn text(&self) -> String {
        decode_text(&self.body, self.header("content-type"))
    }
}

/// Decode a document body. A charset in `Content-Type` wins. Otherwise valid
/// UTF-8 (including a BOM) is used, and bytes that are not UTF-8 are read as
/// windows-1252, which is what HTML requires for unlabeled legacy pages.
pub fn decode_text(body: &[u8], content_type: Option<&str>) -> String {
    let declared = content_type.and_then(charset_label);
    let sniffed = sniff_meta_charset(body);
    let charset = declared.or(sniffed).unwrap_or("utf-8");
    if charset == "utf-8" || charset == "utf8" {
        let bytes = body.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(body);
        if let Ok(text) = std::str::from_utf8(bytes) {
            return text.to_string();
        }
    }
    decode_windows_1252(body)
}

fn charset_label(content_type: &str) -> Option<&str> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some(value) = part
            .split_once('=')
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
            .map(|(_, value)| value.trim().trim_matches('"').trim())
        else {
            continue;
        };
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn sniff_meta_charset(body: &[u8]) -> Option<&str> {
    let head = &body[..body.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head) else {
        return None;
    };
    let lower = text.to_ascii_lowercase();
    let marker = "charset=";
    let start = lower.find(marker)? + marker.len();
    let rest = lower[start..].trim_start_matches([' ', '"', '\'']);
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .unwrap_or(rest.len());
    let label = &rest[..end];
    match label {
        "utf-8" | "utf8" => Some("utf-8"),
        "windows-1252" | "iso-8859-1" | "latin1" | "us-ascii" => Some("windows-1252"),
        _ => None,
    }
}

fn decode_windows_1252(body: &[u8]) -> String {
    const EXTRA: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    body.iter()
        .map(|byte| match byte {
            0x00..=0x7F | 0xA0..=0xFF => char::from(*byte),
            0x80..=0x9F => EXTRA[(*byte - 0x80) as usize],
        })
        .collect()
}

pub fn get(url: &Url) -> Result<Response, NetError> {
    fetch("GET", url, None, &[], 0, CacheMode::Use, None, true, None, &[])
}

/// A CORS `fetch` or XHR. `origin` is the page's serialized origin, which
/// Chromium sends so the server can echo `Access-Control-Allow-Origin`.
pub fn get_with_origin(url: &Url, initiator_host: &str, origin: &str) -> Result<Response, NetError> {
    get_with_origin_headers(url, initiator_host, origin, &[])
}

/// Like [`get_with_origin`], with request headers from `fetch` init.
pub fn get_with_origin_headers(
    url: &Url,
    initiator_host: &str,
    origin: &str,
    headers: &[(&str, &str)],
) -> Result<Response, NetError> {
    fetch(
        "GET",
        url,
        None,
        &[],
        0,
        CacheMode::Use,
        Some(initiator_host),
        false,
        Some(origin),
        headers,
    )
}

/// GET issued by a document. `initiator_host` is that document's host, so
/// `SameSite` can tell a cross-site subresource from a same-site one.
/// Top-level navigation from a page, so `SameSite=Lax` is sent and `Strict` is not
/// when the initiator site differs.
pub fn navigate_from(url: &Url, initiator_host: &str) -> Result<Response, NetError> {
    fetch(
        "GET",
        url,
        None,
        &[],
        0,
        CacheMode::Use,
        Some(initiator_host),
        true,
        None,
        &[],
    )
}

pub fn get_from(url: &Url, initiator_host: &str) -> Result<Response, NetError> {
    fetch(
        "GET",
        url,
        None,
        &[],
        0,
        CacheMode::Use,
        Some(initiator_host),
        false,
        None,
        &[],
    )
}

/// GET that ignores a fresh cache entry. The reload button uses this.
pub fn reload(url: &Url) -> Result<Response, NetError> {
    fetch(
        "GET",
        url,
        None,
        &[],
        0,
        CacheMode::Bypass,
        None,
        true,
        None,
        &[],
    )
}

pub fn post(url: &Url, content_type: &str, body: &[u8]) -> Result<Response, NetError> {
    fetch(
        "POST",
        url,
        Some(content_type),
        body,
        0,
        CacheMode::Bypass,
        None,
        true,
        None,
        &[],
    )
}

fn fetch(
    method: &str,
    url: &Url,
    content_type: Option<&str>,
    body: &[u8],
    redirects: u8,
    cache_mode: CacheMode,
    initiator: Option<&str>,
    top_level: bool,
    origin: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<Response, NetError> {
    if redirects > 8 {
        return Err(NetError::TooManyRedirects);
    }
    if !url.is_network() {
        return Err(NetError::UnsupportedScheme(url.scheme.clone()));
    }
    if url.host.is_empty() {
        return Err(NetError::InvalidUrl);
    }
    let key = cache::cache_key(method, url);
    if method == "GET" && cache_mode == CacheMode::Use {
        if let Some(hit) = with_profile(|profile| profile.cache.get(&key)) {
            return Ok(hit);
        }
        if disk_ok(url)
            && let Some(hit) = cache::load_disk(&disk_dir(), &key)
        {
            let stored = hit.clone();
            if let Some(max_age) = cache::max_age(&stored.headers) {
                with_profile(|profile| profile.cache.insert(key.clone(), stored, max_age));
            }
            return Ok(hit);
        }
    }
    let top_host = if top_level {
        url.host.as_str()
    } else {
        initiator.unwrap_or(url.host.as_str())
    };
    let cookie = with_profile(|profile| {
        profile.cookies.header_for_request(&cookies::CookieRequest {
            url,
            initiator_host: initiator,
            top_level,
            method,
            top_level_host: Some(top_host),
        })
    });
    let port = url.effective_port().unwrap_or(80);
    let request = build_request(
        method,
        url,
        content_type,
        body,
        cookie.as_deref(),
        origin,
        headers,
    );
    let mut parsed = if url.scheme == "https" {
        https_exchange(&url.host, port, &request, method, url)?
    } else {
        let raw = tcp_exchange(&url.host, port, &request)?;
        parse_http_message(&raw)?
    };
    parsed.body = decode_body(&parsed.headers, parsed.body)?;
    parsed
        .headers
        .retain(|(name, _)| name != "content-encoding");
    with_profile(|profile| profile.cookies.ingest_for(url, &parsed.headers, top_host));
    if matches!(parsed.status, 301 | 302 | 303 | 307 | 308)
        && let Some(location) = header(&parsed.headers, "location")
    {
        let next = url
            .resolve(location)
            .map_err(|_| NetError::InvalidRedirect)?;
        let switch_to_get = matches!(parsed.status, 301 | 302 | 303);
        if switch_to_get {
            return fetch(
                "GET",
                &next,
                None,
                &[],
                redirects + 1,
                cache_mode,
                initiator,
                top_level,
                origin,
                headers,
            );
        }
        return fetch(
            method,
            &next,
            content_type,
            body,
            redirects + 1,
            cache_mode,
            initiator,
            top_level,
            origin,
            headers,
        );
    }
    let response = Response {
        status: parsed.status,
        headers: parsed.headers,
        body: parsed.body,
        url: url.clone(),
    };
    if method == "GET"
        && response.status == 200
        && let Some(max_age) = cache::max_age(&response.headers)
    {
        with_profile(|profile| profile.cache.insert(key.clone(), response.clone(), max_age));
        if disk_ok(url) {
            cache::store_disk(&disk_dir(), &key, &response, max_age);
        }
    }
    Ok(response)
}

fn header_value<'a>(headers: &'a [(&str, &str)], name: &str) -> Option<&'a str> {
    headers.iter().rev().find_map(|(key, value)| {
        (key.eq_ignore_ascii_case(name)
            && !value.chars().any(|ch| ch == '\r' || ch == '\n'))
        .then_some(*value)
    })
}

fn header_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn build_request(
    method: &str,
    url: &Url,
    content_type: Option<&str>,
    body: &[u8],
    cookie: Option<&str>,
    origin: Option<&str>,
    headers: &[(&str, &str)],
) -> Vec<u8> {
    let host = match url.port {
        Some(port) if Url::default_port(&url.scheme) != Some(port) => {
            format!("{}:{port}", url.host)
        }
        _ => url.host.clone(),
    };
    let mut head = format!(
        "{method} {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nSec-CH-UA: {SEC_CH_UA}\r\nSec-CH-UA-Mobile: {SEC_CH_UA_MOBILE}\r\nSec-CH-UA-Platform: {SEC_CH_UA_PLATFORM}\r\nAccept: {accept}\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nConnection: close\r\n",
        url.request_target(),
        accept = header_value(headers, "accept")
            .unwrap_or("text/html,application/xhtml+xml,image/*,text/css,*/*;q=0.1"),
    );
    if let Some(content_type) = content_type {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    if let Some(cookie) = cookie {
        head.push_str("Cookie: ");
        head.push_str(cookie);
        head.push_str("\r\n");
    }
    if let Some(origin) = origin {
        head.push_str("Origin: ");
        head.push_str(origin);
        head.push_str("\r\nReferer: ");
        head.push_str(origin);
        if !origin.ends_with('/') {
            head.push('/');
        }
        head.push_str("\r\n");
    }
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("accept")
            || name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("cookie")
            || name.eq_ignore_ascii_case("origin")
            || name.eq_ignore_ascii_case("referer")
        {
            continue;
        }
        if !header_name_ok(name) || value.chars().any(|ch| ch == '\r' || ch == '\n') {
            continue;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut request = head.into_bytes();
    request.extend_from_slice(body);
    request
}

fn tcp_exchange(host: &str, port: u16, request: &[u8]) -> Result<Vec<u8>, NetError> {
    let mut stream = TcpStream::connect((host, port))?;
    prepare_socket(&stream);
    exchange(&mut stream, request)
}

fn https_exchange(
    host: &str,
    port: u16,
    request: &[u8],
    method: &str,
    url: &Url,
) -> Result<RawHttp, NetError> {
    match https_once(host, port, request, method, url, true) {
        Ok(message) => Ok(message),
        Err(error) => {
            if error_is_http2(&error) {
                https_once(host, port, request, method, url, false)
            } else {
                Err(error)
            }
        }
    }
}

fn error_is_http2(error: &NetError) -> bool {
    matches!(error, NetError::Tls(message) if message.starts_with("http2"))
}

fn https_once(
    host: &str,
    port: u16,
    request: &[u8],
    method: &str,
    url: &Url,
    allow_h2: bool,
) -> Result<RawHttp, NetError> {
    let mut socket = TcpStream::connect((host, port))?;
    prepare_socket(&socket);
    let config = tls_config(allow_h2)?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| NetError::Tls(format!("invalid server name {host}")))?;
    let mut connection =
        ClientConnection::new(config, name).map_err(|error| NetError::Tls(error.to_string()))?;
    while connection.is_handshaking() {
        connection
            .complete_io(&mut socket)
            .map_err(|error| NetError::Tls(error.to_string()))?;
    }
    let alpn = connection.alpn_protocol().map(|protocol| protocol.to_vec());
    let mut tls = rustls::Stream::new(&mut connection, &mut socket);
    if allow_h2 && alpn.as_deref() == Some(b"h2".as_slice()) {
        let cookie = header_value_from_request(request, "cookie");
        let origin = header_value_from_request(request, "origin");
        let referer = header_value_from_request(request, "referer");
        return http2::roundtrip(
            &mut tls,
            method,
            url,
            cookie.as_deref(),
            origin.as_deref(),
            referer.as_deref(),
        )
            .map(|message| RawHttp {
                status: message.status,
                headers: message.headers,
                body: message.body,
            })
            .map_err(|error| NetError::Tls(format!("http2: {error}")));
    }
    let raw = exchange(&mut tls, request)?;
    parse_http_message(&raw)
}

fn header_value_from_request(request: &[u8], name: &str) -> Option<String> {
    let text = std::str::from_utf8(request).ok()?;
    let head = text.split("\r\n\r\n").next()?;
    header(&header_pairs(head), name).map(str::to_string)
}

fn prepare_socket(stream: &TcpStream) {
    let timeout = Some(Duration::from_secs(20));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let _ = stream.set_nodelay(true);
}

fn tls_config(allow_h2: bool) -> Result<Arc<ClientConfig>, NetError> {
    static H2: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static HTTP1: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let slot = if allow_h2 { &H2 } else { &HTTP1 };
    if let Some(config) = slot.get() {
        return Ok(Arc::clone(config));
    }
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = if allow_h2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    let config = Arc::new(config);
    let _ = slot.set(Arc::clone(&config));
    Ok(config)
}

fn disk_dir() -> std::path::PathBuf {
    std::env::var_os("MITHU_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mithu-http-cache"))
}

fn disk_ok(url: &Url) -> bool {
    !matches!(url.host.as_str(), "localhost" | "127.0.0.1" | "::1")
}

fn decode_body(headers: &[(String, String)], mut body: Vec<u8>) -> Result<Vec<u8>, NetError> {
    let Some(encoding) = header(headers, "content-encoding") else {
        return Ok(body);
    };
    let mut codings = encoding
        .split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty() && part != "identity")
        .collect::<Vec<_>>();
    codings.reverse();
    for coding in codings {
        body = match coding.as_str() {
            "gzip" | "x-gzip" => inflate_gzip(&body)?,
            "deflate" => inflate_deflate(&body)?,
            "br" => inflate_brotli(&body)?,
            "zstd" => inflate_zstd(&body)?,
            _ => {
                return Err(NetError::Io(format!(
                    "unsupported content-encoding {coding}"
                )));
            }
        };
    }
    Ok(body)
}

fn inflate_gzip(body: &[u8]) -> Result<Vec<u8>, NetError> {
    let mut decoder = flate2::read::GzDecoder::new(body);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| NetError::Io(error.to_string()))?;
    Ok(out)
}

fn inflate_deflate(body: &[u8]) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    if flate2::read::ZlibDecoder::new(body)
        .read_to_end(&mut out)
        .is_ok()
    {
        return Ok(out);
    }
    out.clear();
    flate2::read::DeflateDecoder::new(body)
        .read_to_end(&mut out)
        .map_err(|error| NetError::Io(error.to_string()))?;
    Ok(out)
}

fn inflate_zstd(body: &[u8]) -> Result<Vec<u8>, NetError> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(body)
        .map_err(|error| NetError::Io(error.to_string()))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| NetError::Io(error.to_string()))?;
    Ok(out)
}

fn inflate_brotli(body: &[u8]) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    brotli::Decompressor::new(body, 4096)
        .read_to_end(&mut out)
        .map_err(|error| NetError::Io(error.to_string()))?;
    Ok(out)
}

fn exchange(stream: &mut (impl Read + Write), request: &[u8]) -> Result<Vec<u8>, NetError> {
    stream.write_all(request)?;
    stream.flush()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        if buf.len() > 10_000_000 {
            return Err(NetError::TooLarge);
        }
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(size) => {
                buf.extend_from_slice(&tmp[..size]);
                if message_complete(&buf) {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if buf.is_empty() {
                    return Err(NetError::Timeout);
                }
                break;
            }
            Err(error) => return Err(NetError::Io(error.to_string())),
        }
    }
    if buf.is_empty() {
        return Err(NetError::EmptyResponse);
    }
    Ok(buf)
}

struct RawHttp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn message_complete(buf: &[u8]) -> bool {
    let Some(separator) = find_slice(buf, b"\r\n\r\n") else {
        return false;
    };
    let Ok(head) = std::str::from_utf8(&buf[..separator]) else {
        return false;
    };
    let headers = header_pairs(head);
    let body = &buf[separator + 4..];
    if chunked(&headers) {
        return decode_chunked(body).ok().flatten().is_some();
    }
    if let Some(length) = content_length(&headers) {
        return body.len() >= length;
    }
    false
}

fn parse_http_message(buf: &[u8]) -> Result<RawHttp, NetError> {
    let separator = find_slice(buf, b"\r\n\r\n").ok_or(NetError::BadResponse)?;
    let head = std::str::from_utf8(&buf[..separator]).map_err(|_| NetError::BadResponse)?;
    let mut lines = head.split("\r\n");
    let status = parse_status(lines.next().ok_or(NetError::BadResponse)?)?;
    let headers = header_pairs(head);
    let body_bytes = &buf[separator + 4..];
    let body = if chunked(&headers) {
        decode_chunked(body_bytes)?.ok_or(NetError::BadResponse)?
    } else if let Some(length) = content_length(&headers) {
        let length = length.min(body_bytes.len());
        body_bytes[..length].to_vec()
    } else {
        body_bytes.to_vec()
    };
    Ok(RawHttp {
        status,
        headers,
        body,
    })
}

fn parse_status(line: &str) -> Result<u16, NetError> {
    let mut parts = line.split_whitespace();
    let _version = parts.next().ok_or(NetError::BadResponse)?;
    parts
        .next()
        .ok_or(NetError::BadResponse)?
        .parse()
        .map_err(|_| NetError::BadResponse)
}

fn header_pairs(head: &str) -> Vec<(String, String)> {
    head.split("\r\n")
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn chunked(headers: &[(String, String)]) -> bool {
    header(headers, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
}

fn content_length(headers: &[(String, String)]) -> Option<usize> {
    header(headers, "content-length")?.parse().ok()
}

fn decode_chunked(mut data: &[u8]) -> Result<Option<Vec<u8>>, NetError> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = find_slice(data, b"\r\n") else {
            return Ok(None);
        };
        let line = std::str::from_utf8(&data[..line_end]).map_err(|_| NetError::BadResponse)?;
        let size_hex = line.split(';').next().unwrap_or("").trim();
        if size_hex.is_empty() {
            return Err(NetError::BadResponse);
        }
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| NetError::BadResponse)?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(Some(out));
        }
        if data.len() < size + 2 {
            return Ok(None);
        }
        out.extend_from_slice(&data[..size]);
        if &data[size..size + 2] != b"\r\n" {
            return Err(NetError::BadResponse);
        }
        data = &data[size + 2..];
    }
}

fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_advertise_mithu_after_chrome() {
        let url = Url::parse("https://example.com/a").unwrap();
        let bytes = build_request("GET", &url, None, b"", None, None, &[]);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains(&format!("User-Agent: {USER_AGENT}")));
        assert!(text.contains("Mithu/0.1.0"));
        assert!(text.contains("Chrome/157.0.0.0"));
        assert!(text.contains(&format!("Sec-CH-UA: {SEC_CH_UA}")));
        assert!(text.contains("Sec-CH-UA-Mobile: ?0"));
        assert!(text.contains("Sec-CH-UA-Platform: \"Linux\""));
        let cors = build_request(
            "GET",
            &url,
            None,
            b"",
            None,
            Some("https://www.prothomalo.com"),
            &[("Accept", "application/json")],
        );
        let cors = String::from_utf8(cors).unwrap();
        assert!(cors.contains("Referer: https://www.prothomalo.com/\r\n"));
        assert!(cors.contains("Accept: application/json\r\n"));
        let encoded = http2::encode_request_for_test("GET", &url, None, None);
        let ua = USER_AGENT.as_bytes();
        assert!(encoded.windows(ua.len()).any(|window| window == ua));
    }

    #[test]
    fn windows_1252_bytes_become_characters() {
        let text = decode_text(
            &[0x63, 0x61, 0x66, 0xE9],
            Some("text/html; charset=windows-1252"),
        );
        assert_eq!(text, "café");
        let utf8 = decode_text("বাংলা".as_bytes(), None);
        assert_eq!(utf8, "বাংলা");
    }

    #[test]
    fn parses_content_length_and_ignores_extra_bytes() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhelloEXTRA";
        let message = parse_http_message(raw).unwrap();
        assert_eq!(message.status, 200);
        assert_eq!(message.body, b"hello");
        assert!(message_complete(raw));
    }

    #[test]
    fn zstd_body_is_inflated() {
        let encoded = ruzstd::encoding::compress_to_vec(
            &b"hello zstd"[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let body = decode_body(&[("content-encoding".into(), "zstd".into())], encoded).unwrap();
        assert_eq!(body, b"hello zstd");
    }

    #[test]
    fn gzip_body_is_inflated() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let mut encoded = Vec::new();
            {
                let mut encoder =
                    flate2::write::GzEncoder::new(&mut encoded, flate2::Compression::fast());
                encoder.write_all(b"hello gzip").unwrap();
                encoder.finish().unwrap();
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                encoded.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&encoded).unwrap();
        });
        let url = Url::parse(&format!("http://127.0.0.1:{port}/gz")).unwrap();
        let response = get(&url).unwrap();
        assert_eq!(response.text(), "hello gzip");
        server.join().unwrap();
    }

    #[test]
    fn parses_chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let message = parse_http_message(raw).unwrap();
        assert_eq!(message.body, b"hello");
    }

    #[test]
    fn cookie_is_stored_and_a_fresh_response_is_reused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buf = [0u8; 4096];
                let size = stream.read(&mut buf).unwrap_or(0);
                seen.push(String::from_utf8_lossy(&buf[..size]).into_owned());
                let body = if index == 0 {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nSet-Cookie: sid{port}=abc; Path=/\r\n\r\npage"
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nCache-Control: max-age=120\r\n\r\ncached".to_string()
                };
                stream.write_all(body.as_bytes()).unwrap();
            }
            seen
        });
        let url = Url::parse(&format!("http://127.0.0.1:{port}/cookie")).unwrap();
        let first = get(&url).unwrap();
        assert_eq!(first.text(), "page");
        let second = get(&url).unwrap();
        assert_eq!(second.text(), "cached");
        let third = get(&url).unwrap();
        assert_eq!(third.text(), "cached");
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2, "the third load should come from the cache");
        assert!(!seen[0].contains("Cookie:"), "{:?}", seen[0]);
        assert!(seen[1].contains(&format!("sid{port}=abc")), "{:?}", seen[1]);
        let fresh = reload(&url);
        assert!(
            fresh.is_err(),
            "reload bypasses the cache and the server is gone"
        );
    }
}
