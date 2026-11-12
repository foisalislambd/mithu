//! A small WHATWG-inspired URL parser.
//!
//! It covers the subset a browser needs to navigate: absolute HTTP(S) and
//! `file`/`about` URLs, percent-decoding, and relative resolution against a base.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    MissingScheme,
    Invalid,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty url"),
            Self::MissingScheme => write!(f, "url is missing a scheme"),
            Self::Invalid => write!(f, "invalid url"),
        }
    }
}

impl std::error::Error for UrlError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

impl Url {
    pub fn parse(input: &str) -> Result<Self, UrlError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(UrlError::Empty);
        }
        let scheme_hint = split_scheme(input).map(|(scheme, _)| scheme.to_ascii_lowercase());
        if scheme_hint.as_deref() != Some("data")
            && scheme_hint.as_deref() != Some("blob")
            && let Some((body, fragment)) = input.split_once('#')
        {
            let mut url = Self::parse(body)?;
            url.fragment = Some(fragment.to_string());
            return Ok(url);
        }
        let (scheme, after) = split_scheme(input).ok_or(UrlError::MissingScheme)?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "data" || scheme == "blob" {
            return Ok(Self {
                scheme,
                host: String::new(),
                port: None,
                path: after.to_string(),
                query: None,
                fragment: None,
            });
        }
        if scheme == "about" {
            let path = after.strip_prefix("//").unwrap_or(after);
            return Ok(Self {
                scheme,
                host: String::new(),
                port: None,
                path: path.to_string(),
                query: None,
                fragment: None,
            });
        }

        let (host, port, path_and_query) = if let Some(rest) = after.strip_prefix("//") {
            if rest.is_empty() {
                (String::new(), None, String::from("/"))
            } else if rest.starts_with('/') {
                (String::new(), None, strip_fragment(rest).to_string())
            } else {
                let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let authority = &rest[..auth_end];
                let after_auth = strip_fragment(&rest[auth_end..]);
                let (host, port) = split_host_port(authority)?;
                (host.to_ascii_lowercase(), port, after_auth.to_string())
            }
        } else if scheme == "file" {
            (String::new(), None, strip_fragment(after).to_string())
        } else {
            return Err(UrlError::Invalid);
        };

        let (path, query) = split_path_query(&path_and_query);
        let path = if path.is_empty() {
            "/".to_string()
        } else {
            normalize_path(&percent_decode(path))
        };

        Ok(Self {
            scheme,
            host,
            port,
            path,
            query: query.map(str::to_string),
            fragment: None,
        })
    }

    pub fn resolve(&self, reference: &str) -> Result<Self, UrlError> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Ok(self.clone());
        }
        if split_scheme(reference).is_some() {
            return Self::parse(reference);
        }
        if let Some(rest) = reference.strip_prefix("//") {
            return Self::parse(&format!("{}://{rest}", self.scheme));
        }
        if let Some(fragment) = reference.strip_prefix('#') {
            let mut url = self.clone();
            url.fragment = Some(fragment.to_string());
            return Ok(url);
        }
        let (reference, fragment) = match reference.split_once('#') {
            Some((body, fragment)) => (body, Some(fragment.to_string())),
            None => (reference, None),
        };
        if reference.is_empty() {
            let mut url = self.clone();
            if fragment.is_some() {
                url.fragment = fragment;
            }
            return Ok(url);
        }

        let no_frag = reference.split('#').next().unwrap_or(reference);
        if let Some(query) = no_frag.strip_prefix('?') {
            return Ok(Self {
                query: Some(query.to_string()),
                fragment,
                ..self.clone()
            });
        }

        let (path_ref, query) = split_path_query(no_frag);
        let joined = if path_ref.starts_with('/') {
            percent_decode(path_ref)
        } else {
            format!("{}{}", directory_of(&self.path), percent_decode(path_ref))
        };
        Ok(Self {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path: normalize_path(&joined),
            query: query.map(str::to_string),
            fragment,
        })
    }

    pub fn default_port(scheme: &str) -> Option<u16> {
        match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        }
    }

    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| Self::default_port(&self.scheme))
    }

    pub fn is_network(&self) -> bool {
        self.scheme == "http" || self.scheme == "https"
    }

    /// Path plus query, percent-encoded, suitable for an HTTP request target.
    pub fn request_target(&self) -> String {
        let path = encode_path(&self.path);
        match &self.query {
            Some(query) => format!("{path}?{query}"),
            None => path,
        }
    }

    pub fn to_display(&self) -> String {
        match self.scheme.as_str() {
            "about" => format!("about:{}", self.path),
            "file" => format!("file://{}", self.path),
            _ => {
                let mut out = format!("{}://{}", self.scheme, self.host);
                if let Some(port) = self.port
                    && Self::default_port(&self.scheme) != Some(port)
                {
                    out.push(':');
                    out.push_str(&port.to_string());
                }
                out.push_str(&self.path);
                if let Some(query) = &self.query {
                    out.push('?');
                    out.push_str(query);
                }
                if let Some(fragment) = &self.fragment {
                    out.push('#');
                    out.push_str(fragment);
                }
                out
            }
        }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display())
    }
}

fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b':' {
            return Some((&input[..i], &input[i + 1..]));
        }
        if !(c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.') {
            return None;
        }
        i += 1;
    }
    None
}

fn strip_fragment(input: &str) -> &str {
    input.split('#').next().unwrap_or(input)
}

fn split_path_query(input: &str) -> (&str, Option<&str>) {
    match input.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (input, None),
    }
}

fn split_host_port(authority: &str) -> Result<(String, Option<u16>), UrlError> {
    if authority.is_empty() {
        return Ok((String::new(), None));
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or(UrlError::Invalid)?;
        let host = rest[..end].to_string();
        let after = &rest[end + 1..];
        if let Some(port) = after.strip_prefix(':') {
            if port.is_empty() {
                return Err(UrlError::Invalid);
            }
            let port = port.parse().map_err(|_| UrlError::Invalid)?;
            return Ok((host, Some(port)));
        }
        if after.is_empty() {
            return Ok((host, None));
        }
        return Err(UrlError::Invalid);
    }
    if let Some((host, port)) = authority.rsplit_once(':')
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
    {
        let port = port.parse().map_err(|_| UrlError::Invalid)?;
        return Ok((host.to_string(), Some(port)));
    }
    Ok((authority.to_string(), None))
}

fn directory_of(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..=index].to_string(),
        None => "/".to_string(),
    }
}

fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            parts.pop();
            continue;
        }
        parts.push(segment);
    }
    let mut out = String::new();
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if path.ends_with('/') && out != "/" && !out.ends_with('/') {
        out.push('/');
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &input[i + 1..i + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'=' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_http_url() {
        let url = Url::parse("HTTPS://Example.COM:8443/a/b?q=1#frag").unwrap();
        assert_eq!(url.scheme, "https");
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, Some(8443));
        assert_eq!(url.path, "/a/b");
        assert_eq!(url.query.as_deref(), Some("q=1"));
        assert_eq!(url.fragment.as_deref(), Some("frag"));
        assert_eq!(
            Url::parse("https://example.com/a,b/c:d").unwrap().request_target(),
            "/a,b/c:d"
        );
        assert_eq!(url.to_display(), "https://example.com:8443/a/b?q=1#frag");
        assert_eq!(url.request_target(), "/a/b?q=1");
    }

    #[test]
    fn parses_file_url_with_empty_authority() {
        let url = Url::parse("file:///tmp/a%20b.html").unwrap();
        assert_eq!(url.host, "");
        assert_eq!(url.path, "/tmp/a b.html");
    }

    #[test]
    fn resolves_relative_paths() {
        let base = Url::parse("https://example.com/a/b/c").unwrap();
        assert_eq!(
            base.resolve("../d").unwrap().to_display(),
            "https://example.com/a/d"
        );
        assert_eq!(
            base.resolve("/x").unwrap().to_display(),
            "https://example.com/x"
        );
        assert_eq!(
            base.resolve("?q=2").unwrap().to_display(),
            "https://example.com/a/b/c?q=2"
        );
        assert_eq!(
            base.resolve("https://other.test/z").unwrap().host,
            "other.test"
        );
    }
}
