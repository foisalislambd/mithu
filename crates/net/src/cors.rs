// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from Servo (MPL-2.0):
// - components/script/dom/fetch/headers.rs (`is_forbidden_request_header`,
//   `is_forbidden_response_header`)
// - components/net/fetch/methods.rs (step 14, CORS-exposed header-name list)

//! Fetch's forbidden headers and the CORS-filtered response header list.
//!
//! Servo keeps these checks in the fetch pipeline. Mithu's HTTP client is
//! smaller, so the same rules run at the script boundary: a forbidden request
//! header is dropped before the bytes go out, and a cross-origin response
//! only exposes the safelist plus `Access-Control-Expose-Headers`.

/// <https://fetch.spec.whatwg.org/#forbidden-request-header>
pub fn is_forbidden_request_header(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "accept-charset",
        "accept-encoding",
        "access-control-request-headers",
        "access-control-request-method",
        "connection",
        "content-length",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "expect",
        "host",
        "keep-alive",
        "origin",
        "referer",
        "set-cookie",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "via",
        "access-control-request-private-network",
    ];
    if NAMES.iter().any(|header| name.eq_ignore_ascii_case(header)) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.starts_with("sec-") || lower.starts_with("proxy-")
}

/// <https://fetch.spec.whatwg.org/#forbidden-response-header-name>
pub fn is_forbidden_response_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("set-cookie") || name.eq_ignore_ascii_case("set-cookie2")
}

/// CORS-safelisted response-header names.
/// <https://fetch.spec.whatwg.org/#cors-safelisted-response-header-name>
fn is_cors_safelisted_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cache-control"
            | "content-language"
            | "content-length"
            | "content-type"
            | "expires"
            | "last-modified"
            | "pragma"
    )
}

/// Headers a script may read.
///
/// A same-origin (basic) response exposes every header except the forbidden
/// response names. A CORS response exposes the safelist, plus the names in
/// `Access-Control-Expose-Headers`. Servo's step 14.1.2 does not treat `*` as
/// "every header" when credentials mode is `include`.
pub fn visible_response_headers<'a>(
    headers: &'a [(String, String)],
    same_origin: bool,
    credentials_include: bool,
) -> Vec<(&'a str, &'a str)> {
    if same_origin {
        return headers
            .iter()
            .filter(|(name, _)| !is_forbidden_response_header(name))
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
    }
    let exposed = headers.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case("access-control-expose-headers")
            .then_some(value.as_str())
    });
    let star = exposed.is_some_and(|list| {
        !credentials_include && list.split(',').any(|name| name.trim() == "*")
    });
    headers
        .iter()
        .filter(|(name, _)| {
            if is_forbidden_response_header(name) {
                return false;
            }
            if star || is_cors_safelisted_response_header(name) {
                return true;
            }
            exposed.is_some_and(|list| {
                list.split(',')
                    .any(|item| item.trim().eq_ignore_ascii_case(name))
            })
        })
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// `*` as `Access-Control-Allow-Origin` is not a match when the request
/// asked for credentials. Servo fails that fetch as a network error.
pub fn credentials_allow_origin(allow: Option<&str>, credentials_include: bool) -> bool {
    match allow.map(str::trim) {
        Some("*") => !credentials_include,
        Some(_) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_request_headers_match_servo() {
        assert!(is_forbidden_request_header("Cookie"));
        assert!(is_forbidden_request_header("Sec-Fetch-Mode"));
        assert!(is_forbidden_request_header("Proxy-Authorization"));
        assert!(!is_forbidden_request_header("X-QT-AUTH"));
        assert!(!is_forbidden_request_header("Authorization"));
    }

    #[test]
    fn cors_expose_list_hides_other_names_unless_listed() {
        let headers = vec![
            ("content-type".into(), "application/json".into()),
            ("x-qt-auth".into(), "token".into()),
            ("set-cookie".into(), "a=b".into()),
            ("access-control-expose-headers".into(), "X-QT-AUTH".into()),
        ];
        let visible = visible_response_headers(&headers, false, true);
        let names: Vec<&str> = visible.iter().map(|(name, _)| *name).collect();
        assert!(names.iter().any(|name| name.eq_ignore_ascii_case("content-type")));
        assert!(names.iter().any(|name| name.eq_ignore_ascii_case("x-qt-auth")));
        assert!(!names.iter().any(|name| name.eq_ignore_ascii_case("set-cookie")));
    }

    #[test]
    fn include_credentials_does_not_treat_star_as_every_header() {
        let headers = vec![
            ("x-private".into(), "1".into()),
            ("access-control-expose-headers".into(), "*".into()),
        ];
        let visible = visible_response_headers(&headers, false, true);
        assert!(visible.is_empty());
        let open = visible_response_headers(&headers, false, false);
        assert_eq!(open.len(), 2);
    }
}
