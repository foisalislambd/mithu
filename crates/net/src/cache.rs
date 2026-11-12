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

/// Writes one fresh response under `dir`. Loopback responses stay in memory;
/// the caller decides. The file is a length-prefixed record, not a new crate.
pub fn store_disk(dir: &std::path::Path, key: &str, response: &Response, max_age: u64) {
    if max_age == 0 || response.body.len() > 2_000_000 {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(disk_name(key));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&max_age.to_le_bytes());
    bytes.extend_from_slice(&now_secs().to_le_bytes());
    bytes.extend_from_slice(&response.status.to_le_bytes());
    push_str(&mut bytes, &response.url.to_display());
    push_str(&mut bytes, key);
    bytes.extend_from_slice(&(response.headers.len() as u32).to_le_bytes());
    for (name, value) in &response.headers {
        push_str(&mut bytes, name);
        push_str(&mut bytes, value);
    }
    bytes.extend_from_slice(&(response.body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&response.body);
    let _ = std::fs::write(path, bytes);
}

pub fn load_disk(dir: &std::path::Path, key: &str) -> Option<Response> {
    let path = dir.join(disk_name(key));
    let bytes = std::fs::read(&path).ok()?;
    let mut cursor = 0usize;
    let max_age = read_u64(&bytes, &mut cursor)?;
    let stored_at = read_u64(&bytes, &mut cursor)?;
    if now_secs().saturating_sub(stored_at) >= max_age {
        let _ = std::fs::remove_file(path);
        return None;
    }
    let status = read_u16(&bytes, &mut cursor)?;
    let url = mithu_url::Url::parse(&read_str(&bytes, &mut cursor)?).ok()?;
    let stored_key = read_str(&bytes, &mut cursor)?;
    if stored_key != key {
        return None;
    }
    let count = read_u32(&bytes, &mut cursor)? as usize;
    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        headers.push((
            read_str(&bytes, &mut cursor)?,
            read_str(&bytes, &mut cursor)?,
        ));
    }
    let body_len = read_u32(&bytes, &mut cursor)? as usize;
    if bytes.len() < cursor + body_len {
        return None;
    }
    let body = bytes[cursor..cursor + body_len].to_vec();
    Some(Response {
        status,
        headers,
        body,
        url,
    })
}

fn disk_name(key: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}.http")
}

fn push_str(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let end = cursor.checked_add(8)?;
    let value = u64::from_le_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let end = cursor.checked_add(4)?;
    let value = u32::from_le_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let end = cursor.checked_add(2)?;
    let value = u16::from_le_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_str(bytes: &[u8], cursor: &mut usize) -> Option<String> {
    let len = read_u32(bytes, cursor)? as usize;
    let end = cursor.checked_add(len)?;
    let text = std::str::from_utf8(bytes.get(*cursor..end)?)
        .ok()?
        .to_string();
    *cursor = end;
    Some(text)
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

    #[test]
    fn disk_round_trip_keeps_a_fresh_response() {
        let dir = std::env::temp_dir().join(format!("mithu-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url = Url::parse("https://example.com/cached").unwrap();
        let response = Response {
            status: 200,
            headers: vec![("cache-control".into(), "max-age=60".into())],
            body: b"stored".to_vec(),
            url: url.clone(),
        };
        let key = "GET https://example.com/cached";
        store_disk(&dir, key, &response, 60);
        let loaded = load_disk(&dir, key).unwrap();
        assert_eq!(loaded.body, b"stored");
        store_disk(&dir, "GET https://example.com/old", &response, 0);
        assert!(load_disk(&dir, "GET https://example.com/old").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
