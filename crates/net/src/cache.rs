//! Small in-memory HTTP cache.
//!
//! Chromium's cache lives in `net/http` (HttpCache). Mithu keeps a few fresh
//! GET responses in the browser process so a back navigation or a repeated
//! image fetch does not hit the network again. Only `Cache-Control: max-age`
//! is honored. `no-store` and responses that set cookies are not stored.

use std::time::{SystemTime, UNIX_EPOCH};

use mithu_url::Url;

use crate::Response;

#[derive(Clone, Debug)]
struct Entry {
    response: Response,
    stored_at: u64,
    max_age: u64,
}

#[derive(Clone, Debug)]
pub struct HttpCache {
    entries: Vec<(String, Entry)>,
    limit: usize,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new(32)
    }
}

impl HttpCache {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<Response> {
        let now = now_secs();
        self.entries
            .retain(|(_, entry)| now.saturating_sub(entry.stored_at) < entry.max_age);
        self.entries
            .iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, entry)| entry.response.clone())
    }

    pub fn insert(&mut self, key: String, response: Response, max_age: u64) {
        if max_age == 0 || response.body.len() > 2_000_000 {
            return;
        }
        self.entries.retain(|(stored, _)| stored != &key);
        if self.entries.len() >= self.limit {
            self.entries.remove(0);
        }
        self.entries.push((
            key,
            Entry {
                response,
                stored_at: now_secs(),
                max_age,
            },
        ));
    }
}

pub fn cache_key(method: &str, url: &Url) -> String {
    format!("{method} {}", url.to_display())
}

/// Seconds the response may be reused, when the headers allow storing it.
pub fn max_age(headers: &[(String, String)]) -> Option<u64> {
    let value = headers
        .iter()
        .find(|(name, _)| name == "cache-control")?
        .1
        .to_ascii_lowercase();
    if value.split(',').any(|part| {
        let part = part.trim();
        part == "no-store" || part == "no-cache" || part == "private"
    }) {
        return None;
    }
    if headers.iter().any(|(name, _)| name == "set-cookie") {
        return None;
    }
    for part in value.split(',') {
        let part = part.trim();
        if let Some(age) = part.strip_prefix("max-age=") {
            return age.trim().parse::<u64>().ok().filter(|age| *age > 0);
        }
    }
    None
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

    #[test]
    fn max_age_is_read_and_refused_for_no_store() {
        assert_eq!(
            max_age(&[("cache-control".into(), "max-age=60".into())]),
            Some(60)
        );
        assert_eq!(
            max_age(&[("cache-control".into(), "no-store, max-age=60".into())]),
            None
        );
        assert_eq!(
            max_age(&[
                ("cache-control".into(), "max-age=60".into()),
                ("set-cookie".into(), "a=b".into()),
            ]),
            None
        );
    }

    #[test]
    fn a_stored_response_is_reused() {
        let mut cache = HttpCache::new(2);
        let url = Url::parse("https://example.com/").unwrap();
        let response = Response {
            status: 200,
            headers: vec![("cache-control".into(), "max-age=60".into())],
            body: b"hello".to_vec(),
            url: url.clone(),
        };
        cache.insert(cache_key("GET", &url), response, 60);
        let hit = cache.get(&cache_key("GET", &url)).unwrap();
        assert_eq!(hit.body, b"hello");
    }
}
