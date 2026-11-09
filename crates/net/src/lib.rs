//! Blocking HTTP/1.1 client with TLS.
//!
//! Mithu speaks HTTP itself: request formatting, content-length, chunked
//! bodies, and redirects. TLS is provided by Rustls so the certificate check
//! is a real one, not a hand-rolled cipher.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mithu_url::Url;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

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
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub fn get(url: &Url) -> Result<Response, NetError> {
    get_redirecting(url, 0)
}

fn get_redirecting(url: &Url, redirects: u8) -> Result<Response, NetError> {
    if redirects > 8 {
        return Err(NetError::TooManyRedirects);
    }
    if !url.is_network() {
        return Err(NetError::UnsupportedScheme(url.scheme.clone()));
    }
    if url.host.is_empty() {
        return Err(NetError::InvalidUrl);
    }
    let port = url.effective_port().unwrap_or(80);
    let request = build_request(url);
    let raw = if url.scheme == "https" {
        https_exchange(&url.host, port, &request)?
    } else {
        tcp_exchange(&url.host, port, &request)?
    };
    let parsed = parse_http_message(&raw)?;
    if matches!(parsed.status, 301 | 302 | 303 | 307 | 308)
        && let Some(location) = header(&parsed.headers, "location")
    {
        let next = url.resolve(location).map_err(|_| NetError::InvalidRedirect)?;
        return get_redirecting(&next, redirects + 1);
    }
    Ok(Response {
        status: parsed.status,
        headers: parsed.headers,
        body: parsed.body,
        url: url.clone(),
    })
}

fn build_request(url: &Url) -> String {
    let host = match url.port {
        Some(port) if Url::default_port(&url.scheme) != Some(port) => {
            format!("{}:{port}", url.host)
        }
        _ => url.host.clone(),
    };
    format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mithu/0.1 (Rust)\r\nAccept: text/html,application/xhtml+xml,text/css,*/*;q=0.1\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.request_target()
    )
}

fn tcp_exchange(host: &str, port: u16, request: &str) -> Result<Vec<u8>, NetError> {
    let mut stream = TcpStream::connect((host, port))?;
    prepare_socket(&stream);
    exchange(&mut stream, request)
}

fn https_exchange(host: &str, port: u16, request: &str) -> Result<Vec<u8>, NetError> {
    let mut socket = TcpStream::connect((host, port))?;
    prepare_socket(&socket);
    let config = tls_config()?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| NetError::Tls(format!("invalid server name {host}")))?;
    let mut connection = ClientConnection::new(config, name)
        .map_err(|error| NetError::Tls(error.to_string()))?;
    let mut tls = rustls::Stream::new(&mut connection, &mut socket);
    exchange(&mut tls, request)
}

fn prepare_socket(stream: &TcpStream) {
    let timeout = Some(Duration::from_secs(20));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let _ = stream.set_nodelay(true);
}

fn tls_config() -> Result<Arc<ClientConfig>, NetError> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(Arc::clone(config));
    }
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let config = Arc::new(config);
    let _ = CONFIG.set(Arc::clone(&config));
    Ok(config)
}

fn exchange(stream: &mut (impl Read + Write), request: &str) -> Result<Vec<u8>, NetError> {
    stream.write_all(request.as_bytes())?;
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
    fn parses_content_length_and_ignores_extra_bytes() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhelloEXTRA";
        let message = parse_http_message(raw).unwrap();
        assert_eq!(message.status, 200);
        assert_eq!(message.body, b"hello");
        assert!(message_complete(raw));
    }

    #[test]
    fn parses_chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let message = parse_http_message(raw).unwrap();
        assert_eq!(message.body, b"hello");
    }
}
