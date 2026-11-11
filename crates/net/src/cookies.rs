//! Cookie jar, a small subset of RFC 6265.
//!
//! Chromium stores cookies in the network service (`net/cookies`). Mithu keeps
//! one jar for the browser profile and sends it on later requests to the same
//! site. `HttpOnly` and `SameSite` are accepted and ignored: scripts cannot
//! read the jar, and every navigation is a top-level request.

use std::time::{SystemTime, UNIX_EPOCH};

use mithu_url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            cookies: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Stores every `Set-Cookie` header from a response.
    pub fn ingest(&mut self, url: &Url, headers: &[(String, String)]) {
        let now = now_secs();
        self.cookies
            .retain(|cookie| cookie.expires_at.is_none_or(|expires| expires > now));
        for (name, value) in headers {
            if name != "set-cookie" {
                continue;
            }
            let Some(parsed) = parse_set_cookie(value, url, now) else {
                continue;
            };
            self.cookies.retain(|cookie| !same_slot(cookie, &parsed));
            if parsed.expires_at != Some(0) {
                self.cookies.push(parsed);
            }
        }
    }

    /// `Cookie` header value for `url`, or `None` when nothing matches.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        let now = now_secs();
        let mut pairs = Vec::new();
        for cookie in &self.cookies {
            if cookie.expires_at.is_some_and(|expires| expires <= now) {
                continue;
            }
            if cookie.secure && url.scheme != "https" {
                continue;
            }
            if !domain_matches(cookie, &url.host) || !path_matches(&cookie.path, &url.path) {
                continue;
            }
            pairs.push(format!("{}={}", cookie.name, cookie.value));
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    }
}

pub fn parse_set_cookie(header: &str, url: &Url, now: u64) -> Option<Cookie> {
    let mut parts = header.split(';');
    let pair = parts.next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let mut cookie = Cookie {
        name: name.to_string(),
        value: value.trim().to_string(),
        domain: url.host.to_ascii_lowercase(),
        host_only: true,
        path: default_path(&url.path),
        secure: false,
        expires_at: None,
    };
    for attribute in parts {
        let attribute = attribute.trim();
        if attribute.is_empty() {
            continue;
        }
        let (key, raw) = attribute
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .unwrap_or((attribute, ""));
        match key.to_ascii_lowercase().as_str() {
            "domain" => {
                let domain = raw.trim_start_matches('.').to_ascii_lowercase();
                if host_matches_domain(&url.host, &domain) {
                    cookie.domain = domain;
                    cookie.host_only = false;
                }
            }
            "path" => {
                if raw.starts_with('/') {
                    cookie.path = raw.to_string();
                }
            }
            "secure" => cookie.secure = true,
            "max-age" => match raw.parse::<i64>() {
                Ok(age) if age <= 0 => cookie.expires_at = Some(0),
                Ok(age) => cookie.expires_at = Some(now.saturating_add(age as u64)),
                Err(_) => {}
            },
            _ => {}
        }
    }
    Some(cookie)
}

fn same_slot(left: &Cookie, right: &Cookie) -> bool {
    left.name == right.name
        && left.domain == right.domain
        && left.path == right.path
        && left.host_only == right.host_only
}

fn domain_matches(cookie: &Cookie, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if cookie.host_only {
        host == cookie.domain
    } else {
        host_matches_domain(&host, &cookie.domain)
    }
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    if cookie_path.is_empty() {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    request_path.len() == cookie_path.len()
        || cookie_path.ends_with('/')
        || request_path[cookie_path.len()..].starts_with('/')
}

fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..=index].to_string(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn cookie_is_sent_only_to_the_matching_host_and_path() {
        let mut jar = CookieJar::new();
        let home = url("https://example.com/app/index.html");
        jar.ingest(&home, &[("set-cookie".into(), "sid=abc; Path=/app".into())]);
        assert_eq!(
            jar.header_for(&url("https://example.com/app/next"))
                .as_deref(),
            Some("sid=abc")
        );
        assert!(jar.header_for(&url("https://example.com/other")).is_none());
        assert!(jar.header_for(&url("https://other.test/app")).is_none());
        assert_eq!(
            jar.header_for(&url("http://example.com/app/next"))
                .as_deref(),
            Some("sid=abc")
        );
    }

    #[test]
    fn secure_cookie_stays_on_https_and_max_age_zero_deletes() {
        let mut jar = CookieJar::new();
        let page = url("https://example.com/");
        jar.ingest(
            &page,
            &[("set-cookie".into(), "sid=abc; Secure; Path=/".into())],
        );
        assert!(jar.header_for(&url("http://example.com/")).is_none());
        assert_eq!(jar.header_for(&page).as_deref(), Some("sid=abc"));
        jar.ingest(
            &page,
            &[("set-cookie".into(), "sid=abc; Max-Age=0; Path=/".into())],
        );
        assert!(jar.header_for(&page).is_none());
    }

    #[test]
    fn domain_attribute_covers_subdomains() {
        let mut jar = CookieJar::new();
        jar.ingest(
            &url("https://www.example.com/"),
            &[(
                "set-cookie".into(),
                "theme=light; Domain=example.com; Path=/".into(),
            )],
        );
        assert_eq!(
            jar.header_for(&url("https://docs.example.com/")).as_deref(),
            Some("theme=light")
        );
        assert!(jar.header_for(&url("https://example.org/")).is_none());
    }
}
