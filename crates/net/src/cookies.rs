//! Cookie jar, a small subset of RFC 6265.
//!
//! Chromium stores cookies in the network service (`net/cookies`). Mithu keeps
//! one jar for the browser profile and sends it on later requests to the same
//! site. `Expires` and `SameSite` decide which later request includes the
//! cookie. `document.cookie` reads and writes the same jar, skipping `HttpOnly`.

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
    pub same_site: SameSite,
    /// CHIPS `Partitioned` cookies are stored under the top-level site.
    pub partitioned: bool,
    pub partition_key: String,
    /// `HttpOnly` cookies are sent on requests and hidden from `document.cookie`.
    pub http_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Default,
    Lax,
    Strict,
    None,
}

pub struct CookieRequest<'a> {
    pub url: &'a Url,
    pub initiator_host: Option<&'a str>,
    pub top_level: bool,
    pub method: &'a str,
    /// Top-level site that is loading the document. Partitioned cookies
    /// are sent only when this site matches the key stored at `Set-Cookie`.
    pub top_level_host: Option<&'a str>,
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
    ///
    /// `top_level_host` is the page that caused the response. A `Partitioned`
    /// cookie is keyed to that site, the way Chromium keys CHIPS cookies.
    pub fn ingest(&mut self, url: &Url, headers: &[(String, String)]) {
        self.ingest_for(url, headers, &url.host);
    }

    pub fn ingest_for(&mut self, url: &Url, headers: &[(String, String)], top_level_host: &str) {
        let now = now_secs();
        self.cookies
            .retain(|cookie| cookie.expires_at.is_none_or(|expires| expires > now));
        for (name, value) in headers {
            if name != "set-cookie" {
                continue;
            }
            let Some(parsed) = parse_set_cookie(value, url, now, top_level_host) else {
                continue;
            };
            self.cookies.retain(|cookie| !same_slot(cookie, &parsed));
            if parsed.expires_at != Some(0) {
                self.cookies.push(parsed);
            }
        }
    }

    /// `Cookie` header for an address-bar load. No initiator means the cookie
    /// is not crossing sites, so `SameSite` does not drop it.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        self.header_for_request(&CookieRequest {
            url,
            initiator_host: None,
            top_level: true,
            method: "GET",
            top_level_host: None,
        })
    }

    pub fn header_for_request(&self, request: &CookieRequest<'_>) -> Option<String> {
        let now = now_secs();
        let mut pairs = Vec::new();
        for cookie in &self.cookies {
            if cookie.expires_at.is_some_and(|expires| expires <= now) {
                continue;
            }
            if cookie.secure && request.url.scheme != "https" {
                continue;
            }
            if !domain_matches(cookie, &request.url.host)
                || !path_matches(&cookie.path, &request.url.path)
            {
                continue;
            }
            if !same_site_allows(cookie, request) {
                continue;
            }
            if cookie.partitioned {
                let top = request.top_level_host.unwrap_or(&request.url.host);
                if site_of(top) != cookie.partition_key {
                    continue;
                }
            }
            pairs.push(format!("{}={}", cookie.name, cookie.value));
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    }

    /// `document.cookie` for `url`: every matching cookie except `HttpOnly`.
    pub fn document_cookie(&self, url: &Url) -> String {
        let now = now_secs();
        let mut pairs = Vec::new();
        for cookie in &self.cookies {
            if cookie.http_only || cookie.expires_at.is_some_and(|expires| expires <= now) {
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
        pairs.join("; ")
    }

    /// Stores one `document.cookie` assignment. Scripts cannot set `HttpOnly`.
    pub fn set_document_cookie(&mut self, url: &Url, line: &str) {
        let now = now_secs();
        let Some(mut parsed) = parse_set_cookie(line, url, now, &url.host) else {
            return;
        };
        parsed.http_only = false;
        self.cookies.retain(|cookie| !same_slot(cookie, &parsed));
        if parsed.expires_at != Some(0) {
            self.cookies.push(parsed);
        }
    }
}

pub fn parse_set_cookie(header: &str, url: &Url, now: u64, top_level_host: &str) -> Option<Cookie> {
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
        same_site: SameSite::Default,
        partitioned: false,
        partition_key: String::new(),
        http_only: false,
    };
    let mut max_age_set = false;
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
            "httponly" => cookie.http_only = true,
            "max-age" => match raw.parse::<i64>() {
                Ok(age) if age <= 0 => {
                    cookie.expires_at = Some(0);
                    max_age_set = true;
                }
                Ok(age) => {
                    cookie.expires_at = Some(now.saturating_add(age as u64));
                    max_age_set = true;
                }
                Err(_) => {}
            },
            "expires" if !max_age_set => {
                cookie.expires_at = parse_http_date(raw);
            }
            "samesite" => {
                cookie.same_site = match raw.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                };
            }
            "partitioned" => {
                cookie.partitioned = true;
                cookie.partition_key = site_of(top_level_host);
            }
            _ => {}
        }
    }
    if cookie.same_site == SameSite::None && !cookie.secure {
        return None;
    }
    Some(cookie)
}

fn same_site_allows(cookie: &Cookie, request: &CookieRequest<'_>) -> bool {
    let Some(initiator) = request.initiator_host else {
        return true;
    };
    if site_of(initiator) == site_of(&request.url.host) {
        return true;
    }
    match cookie.same_site {
        SameSite::Strict => false,
        SameSite::Lax | SameSite::Default => {
            request.top_level && request.method.eq_ignore_ascii_case("GET")
        }
        SameSite::None => cookie.secure,
    }
}

fn site_of(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    let parts: Vec<&str> = host.split('.').filter(|part| !part.is_empty()).collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        host
    }
}

fn parse_http_date(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value == "0" {
        return Some(0);
    }
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?;
    let day: u64 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let month = match parts.next()?.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i32 = parts.next()?.parse().ok()?;
    let clock = parts.next()?;
    let mut clock = clock.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    Some(days_from_civil(year, month, day as i32) * 86400 + hour * 3600 + minute * 60 + second)
}

/// Howard Hinnant's civil-from-days, inverted. Days since 1970-01-01.
fn days_from_civil(year: i32, month: u32, day: i32) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let month = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as i64 * 146097 + doe as i64 - 719468;
    days.max(0) as u64
}

fn same_slot(left: &Cookie, right: &Cookie) -> bool {
    left.name == right.name
        && left.domain == right.domain
        && left.path == right.path
        && left.host_only == right.host_only
        && left.partition_key == right.partition_key
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
    fn expires_and_samesite_filter_later_requests() {
        let mut jar = CookieJar::new();
        let page = url("https://example.com/");
        jar.ingest(
            &page,
            &[(
                "set-cookie".into(),
                "old=1; Expires=Thu, 01 Jan 1970 00:00:01 GMT; Path=/".into(),
            )],
        );
        assert!(jar.header_for(&page).is_none(), "a past Expires deletes");
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        jar.ingest(
            &page,
            &[(
                "set-cookie".into(),
                "strict=1; SameSite=Strict; Path=/".into(),
            )],
        );
        jar.ingest(
            &page,
            &[("set-cookie".into(), "lax=1; SameSite=Lax; Path=/".into())],
        );
        let cross = CookieRequest {
            url: &page,
            initiator_host: Some("other.test"),
            top_level: true,
            method: "GET",
            top_level_host: Some("other.test"),
        };
        let header = jar.header_for_request(&cross).unwrap_or_default();
        assert!(!header.contains("strict=1"), "{header}");
        assert!(header.contains("lax=1"), "{header}");
        let sub = CookieRequest {
            url: &page,
            initiator_host: Some("other.test"),
            top_level: false,
            method: "GET",
            top_level_host: Some("other.test"),
        };
        assert!(jar.header_for_request(&sub).is_none());
        let same = CookieRequest {
            url: &url("https://www.example.com/"),
            initiator_host: Some("example.com"),
            top_level: false,
            method: "POST",
            top_level_host: Some("example.com"),
        };
        jar.ingest(
            &page,
            &[(
                "set-cookie".into(),
                "shared=1; Domain=example.com; Path=/".into(),
            )],
        );
        assert!(
            jar.header_for_request(&same)
                .unwrap_or_default()
                .contains("shared=1")
        );
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

    #[test]
    fn partitioned_cookie_stays_on_the_top_level_site() {
        let mut jar = CookieJar::new();
        let cdn = url("https://cdn.example/pixel");
        jar.ingest_for(
            &cdn,
            &[(
                "set-cookie".into(),
                "id=1; Secure; Path=/; SameSite=None; Partitioned".into(),
            )],
            "news.example",
        );
        let on_news = CookieRequest {
            url: &cdn,
            initiator_host: Some("news.example"),
            top_level: false,
            method: "GET",
            top_level_host: Some("news.example"),
        };
        assert_eq!(jar.header_for_request(&on_news).as_deref(), Some("id=1"));
        let on_other = CookieRequest {
            url: &cdn,
            initiator_host: Some("other.example"),
            top_level: false,
            method: "GET",
            top_level_host: Some("other.example"),
        };
        assert!(jar.header_for_request(&on_other).is_none());
    }
}
