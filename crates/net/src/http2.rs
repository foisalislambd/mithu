//! A single-request HTTP/2 client.
//!
//! Chromium's HTTP/2 stack lives in `net/spdy` and QUICHE. Those trees are not
//! linked here. This module speaks just enough of RFC 9113 and HPACK (RFC 7541)
//! to fetch one response over a TLS connection that negotiated `h2`: the
//! connection preface, settings, one HEADERS frame, and the response DATA.
//! Huffman codes are the static table from RFC 7541 Appendix B.

use std::io::{Read, Write};

pub struct Message {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

const HUFFMAN: [(u32, u8); 257] = [
    (0x1ff8, 13),
    (0x7fffd8, 23),
    (0xfffffe2, 28),
    (0xfffffe3, 28),
    (0xfffffe4, 28),
    (0xfffffe5, 28),
    (0xfffffe6, 28),
    (0xfffffe7, 28),
    (0xfffffe8, 28),
    (0xffffea, 24),
    (0x3ffffffc, 30),
    (0xfffffe9, 28),
    (0xfffffea, 28),
    (0x3ffffffd, 30),
    (0xfffffeb, 28),
    (0xfffffec, 28),
    (0xfffffed, 28),
    (0xfffffee, 28),
    (0xfffffef, 28),
    (0xffffff0, 28),
    (0xffffff1, 28),
    (0xffffff2, 28),
    (0x3ffffffe, 30),
    (0xffffff3, 28),
    (0xffffff4, 28),
    (0xffffff5, 28),
    (0xffffff6, 28),
    (0xffffff7, 28),
    (0xffffff8, 28),
    (0xffffff9, 28),
    (0xffffffa, 28),
    (0xffffffb, 28),
    (0x14, 6),
    (0x3f8, 10),
    (0x3f9, 10),
    (0xffa, 12),
    (0x1ff9, 13),
    (0x15, 6),
    (0xf8, 8),
    (0x7fa, 11),
    (0x3fa, 10),
    (0x3fb, 10),
    (0xf9, 8),
    (0x7fb, 11),
    (0xfa, 8),
    (0x16, 6),
    (0x17, 6),
    (0x18, 6),
    (0x0, 5),
    (0x1, 5),
    (0x2, 5),
    (0x19, 6),
    (0x1a, 6),
    (0x1b, 6),
    (0x1c, 6),
    (0x1d, 6),
    (0x1e, 6),
    (0x1f, 6),
    (0x5c, 7),
    (0xfb, 8),
    (0x7ffc, 15),
    (0x20, 6),
    (0xffb, 12),
    (0x3fc, 10),
    (0x1ffa, 13),
    (0x21, 6),
    (0x5d, 7),
    (0x5e, 7),
    (0x5f, 7),
    (0x60, 7),
    (0x61, 7),
    (0x62, 7),
    (0x63, 7),
    (0x64, 7),
    (0x65, 7),
    (0x66, 7),
    (0x67, 7),
    (0x68, 7),
    (0x69, 7),
    (0x6a, 7),
    (0x6b, 7),
    (0x6c, 7),
    (0x6d, 7),
    (0x6e, 7),
    (0x6f, 7),
    (0x70, 7),
    (0x71, 7),
    (0x72, 7),
    (0xfc, 8),
    (0x73, 7),
    (0xfd, 8),
    (0x1ffb, 13),
    (0x7fff0, 19),
    (0x1ffc, 13),
    (0x3ffc, 14),
    (0x22, 6),
    (0x7ffd, 15),
    (0x3, 5),
    (0x23, 6),
    (0x4, 5),
    (0x24, 6),
    (0x5, 5),
    (0x25, 6),
    (0x26, 6),
    (0x27, 6),
    (0x6, 5),
    (0x74, 7),
    (0x75, 7),
    (0x28, 6),
    (0x29, 6),
    (0x2a, 6),
    (0x7, 5),
    (0x2b, 6),
    (0x76, 7),
    (0x2c, 6),
    (0x8, 5),
    (0x9, 5),
    (0x2d, 6),
    (0x77, 7),
    (0x78, 7),
    (0x79, 7),
    (0x7a, 7),
    (0x7b, 7),
    (0x7ffe, 15),
    (0x7fc, 11),
    (0x3ffd, 14),
    (0x1ffd, 13),
    (0xffffffc, 28),
    (0xfffe6, 20),
    (0x3fffd2, 22),
    (0xfffe7, 20),
    (0xfffe8, 20),
    (0x3fffd3, 22),
    (0x3fffd4, 22),
    (0x3fffd5, 22),
    (0x7fffd9, 23),
    (0x3fffd6, 22),
    (0x7fffda, 23),
    (0x7fffdb, 23),
    (0x7fffdc, 23),
    (0x7fffdd, 23),
    (0x7fffde, 23),
    (0xffffeb, 24),
    (0x7fffdf, 23),
    (0xffffec, 24),
    (0xffffed, 24),
    (0x3fffd7, 22),
    (0x7fffe0, 23),
    (0xffffee, 24),
    (0x7fffe1, 23),
    (0x7fffe2, 23),
    (0x7fffe3, 23),
    (0x7fffe4, 23),
    (0x1fffdc, 21),
    (0x3fffd8, 22),
    (0x7fffe5, 23),
    (0x3fffd9, 22),
    (0x7fffe6, 23),
    (0x7fffe7, 23),
    (0xffffef, 24),
    (0x3fffda, 22),
    (0x1fffdd, 21),
    (0xfffe9, 20),
    (0x3fffdb, 22),
    (0x3fffdc, 22),
    (0x7fffe8, 23),
    (0x7fffe9, 23),
    (0x1fffde, 21),
    (0x7fffea, 23),
    (0x3fffdd, 22),
    (0x3fffde, 22),
    (0xfffff0, 24),
    (0x1fffdf, 21),
    (0x3fffdf, 22),
    (0x7fffeb, 23),
    (0x7fffec, 23),
    (0x1fffe0, 21),
    (0x1fffe1, 21),
    (0x3fffe0, 22),
    (0x1fffe2, 21),
    (0x7fffed, 23),
    (0x3fffe1, 22),
    (0x7fffee, 23),
    (0x7fffef, 23),
    (0xfffea, 20),
    (0x3fffe2, 22),
    (0x3fffe3, 22),
    (0x3fffe4, 22),
    (0x7ffff0, 23),
    (0x3fffe5, 22),
    (0x3fffe6, 22),
    (0x7ffff1, 23),
    (0x3ffffe0, 26),
    (0x3ffffe1, 26),
    (0xfffeb, 20),
    (0x7fff1, 19),
    (0x3fffe7, 22),
    (0x7ffff2, 23),
    (0x3fffe8, 22),
    (0x1ffffec, 25),
    (0x3ffffe2, 26),
    (0x3ffffe3, 26),
    (0x3ffffe4, 26),
    (0x7ffffde, 27),
    (0x7ffffdf, 27),
    (0x3ffffe5, 26),
    (0xfffff1, 24),
    (0x1ffffed, 25),
    (0x7fff2, 19),
    (0x1fffe3, 21),
    (0x3ffffe6, 26),
    (0x7ffffe0, 27),
    (0x7ffffe1, 27),
    (0x3ffffe7, 26),
    (0x7ffffe2, 27),
    (0xfffff2, 24),
    (0x1fffe4, 21),
    (0x1fffe5, 21),
    (0x3ffffe8, 26),
    (0x3ffffe9, 26),
    (0xffffffd, 28),
    (0x7ffffe3, 27),
    (0x7ffffe4, 27),
    (0x7ffffe5, 27),
    (0xfffec, 20),
    (0xfffff3, 24),
    (0xfffed, 20),
    (0x1fffe6, 21),
    (0x3fffe9, 22),
    (0x1fffe7, 21),
    (0x1fffe8, 21),
    (0x7ffff3, 23),
    (0x3fffea, 22),
    (0x3fffeb, 22),
    (0x1ffffee, 25),
    (0x1ffffef, 25),
    (0xfffff4, 24),
    (0xfffff5, 24),
    (0x3ffffea, 26),
    (0x7ffff4, 23),
    (0x3ffffeb, 26),
    (0x7ffffe6, 27),
    (0x3ffffec, 26),
    (0x3ffffed, 26),
    (0x7ffffe7, 27),
    (0x7ffffe8, 27),
    (0x7ffffe9, 27),
    (0x7ffffea, 27),
    (0x7ffffeb, 27),
    (0xffffffe, 28),
    (0x7ffffec, 27),
    (0x7ffffed, 27),
    (0x7ffffee, 27),
    (0x7ffffef, 27),
    (0x7fffff0, 27),
    (0x3ffffee, 26),
    (0x3fffffff, 30),
];

const STATIC: &[(&str, &str)] = &[
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate, br, zstd"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

pub fn roundtrip(
    stream: &mut (impl Read + Write),
    method: &str,
    url: &mithu_url::Url,
    cookie: Option<&str>,
) -> Result<Message, String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
    let mut settings = Vec::new();
    settings.extend_from_slice(&4u16.to_be_bytes());
    settings.extend_from_slice(&(1024u32 * 1024).to_be_bytes());
    out.extend(frame(4, 0, 0, &settings));
    let block = encode_request(method, url, cookie);
    let flags = if method.eq_ignore_ascii_case("GET") {
        0x5
    } else {
        0x4
    };
    out.extend(frame(1, flags, 1, &block));
    let window = (16u32 * 1024 * 1024).to_be_bytes();
    out.extend(frame(8, 0, 0, &window));
    out.extend(frame(8, 0, 1, &window));
    stream.write_all(&out).map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;

    let mut decoder = Decoder::new();
    let mut headers = Vec::new();
    let mut body = Vec::new();
    let mut header_block = Vec::new();
    let mut saw_end = false;
    for _ in 0..4096 {
        let header = read_exact(stream, 9)?;
        let length =
            ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize;
        if length > 2_000_000 {
            return Err("http2 frame is too large".into());
        }
        let kind = header[3];
        let flags = header[4];
        let stream_id =
            u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & 0x7fff_ffff;
        let mut payload = read_exact(stream, length)?;
        if kind == 6 {
            out.clear();
            out.extend(frame(6, 1, 0, &payload));
            stream.write_all(&out).map_err(|error| error.to_string())?;
            continue;
        }
        if kind == 4 && flags & 1 == 0 {
            out.clear();
            out.extend(frame(4, 1, 0, &[]));
            stream.write_all(&out).map_err(|error| error.to_string())?;
            stream.flush().map_err(|error| error.to_string())?;
            continue;
        }
        if kind == 7 {
            if saw_end {
                break;
            }
            return Err("http2 goaway".into());
        }
        if stream_id != 1 {
            continue;
        }
        if kind == 3 {
            return Err("http2 reset".into());
        }
        if flags & 0x8 != 0 {
            if payload.is_empty() {
                return Err("http2 padding".into());
            }
            let pad = payload[0] as usize;
            if payload.len() < 1 + pad {
                return Err("http2 padding".into());
            }
            payload = payload[1..payload.len() - pad].to_vec();
        }
        if kind == 1 && flags & 0x20 != 0 {
            if payload.len() < 5 {
                return Err("http2 priority".into());
            }
            payload = payload[5..].to_vec();
        }
        if kind == 1 || kind == 9 {
            header_block.extend_from_slice(&payload);
            if flags & 0x4 != 0 {
                headers = decoder.decode(&header_block)?;
                header_block.clear();
            }
        } else if kind == 0 {
            body.extend_from_slice(&payload);
            if body.len() > 10_000_000 {
                return Err("http2 body is too large".into());
            }
        }
        if (kind == 0 || kind == 1) && flags & 0x1 != 0 {
            saw_end = true;
            break;
        }
    }
    if !saw_end {
        return Err("http2 response ended early".into());
    }
    let status = headers
        .iter()
        .find(|(name, _)| name == ":status")
        .and_then(|(_, value)| value.parse().ok())
        .ok_or_else(|| "http2 response has no :status".to_string())?;
    let headers = headers
        .into_iter()
        .filter(|(name, _)| !name.starts_with(':'))
        .collect();
    Ok(Message {
        status,
        headers,
        body,
    })
}

fn encode_request(method: &str, url: &mithu_url::Url, cookie: Option<&str>) -> Vec<u8> {
    let mut block = Vec::new();
    if method.eq_ignore_ascii_case("GET") {
        block.push(0x82);
    } else if method.eq_ignore_ascii_case("POST") {
        block.push(0x83);
    } else {
        block.extend(literal_indexed(2, method));
    }
    if url.scheme == "http" {
        block.push(0x86);
    } else {
        block.push(0x87);
    }
    if url.request_target() == "/" {
        block.push(0x84);
    } else {
        block.extend(literal_indexed(4, &url.request_target()));
    }
    let authority = match url.port {
        Some(port) if mithu_url::Url::default_port(&url.scheme) != Some(port) => {
            format!("{}:{port}", url.host)
        }
        _ => url.host.clone(),
    };
    block.extend(literal_indexed(1, &authority));
    block.extend(literal(
        "accept",
        "text/html,application/xhtml+xml,image/*,text/css,*/*;q=0.1",
    ));
    block.extend(literal("accept-encoding", "gzip, deflate, br, zstd"));
    block.extend(literal("user-agent", "Mithu/0.1 (Rust)"));
    if let Some(cookie) = cookie {
        block.extend(literal("cookie", cookie));
    }
    block
}

fn literal(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x00];
    push_string(&mut out, name);
    push_string(&mut out, value);
    out
}

fn literal_indexed(index: u8, value: &str) -> Vec<u8> {
    let mut out = vec![index & 0x0f];
    push_string(&mut out, value);
    out
}

fn push_string(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    if bytes.len() < 127 {
        out.push(bytes.len() as u8);
    } else {
        out.push(127);
        let mut rest = bytes.len() - 127;
        while rest >= 128 {
            out.push((rest as u8) | 0x80);
            rest >>= 7;
        }
        out.push(rest as u8);
    }
    out.extend_from_slice(bytes);
}

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let length = payload.len() as u32;
    let mut out = vec![
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
        kind,
        flags,
    ];
    out.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_exact(stream: &mut impl Read, length: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0; length];
    let mut filled = 0;
    while filled < length {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err("http2 connection closed".into()),
            Ok(size) => filled += size,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(buf)
}

struct Decoder {
    dynamic: Vec<(String, String)>,
    max: usize,
}

impl Decoder {
    fn new() -> Self {
        Self {
            dynamic: Vec::new(),
            max: 4096,
        }
    }

    fn decode(&mut self, mut data: &[u8]) -> Result<Vec<(String, String)>, String> {
        let mut headers = Vec::new();
        while !data.is_empty() {
            let byte = data[0];
            if byte & 0xe0 == 0x20 {
                let (size, rest) = read_int(data, 5)?;
                data = rest;
                self.max = size;
                self.evict();
                continue;
            }
            if byte & 0x80 == 0x80 {
                let (index, rest) = read_int(data, 7)?;
                data = rest;
                headers.push(self.lookup(index)?);
                continue;
            }
            let incremental = byte & 0xc0 == 0x40;
            let prefix = if incremental { 6 } else { 4 };
            let (name_index, rest) = read_int(data, prefix)?;
            data = rest;
            let name = if name_index == 0 {
                let (name, rest) = read_string(data)?;
                data = rest;
                name
            } else {
                self.lookup(name_index)?.0
            };
            let (value, rest) = read_string(data)?;
            data = rest;
            if incremental {
                self.insert(name.clone(), value.clone());
            }
            headers.push((name, value));
        }
        Ok(headers)
    }

    fn lookup(&self, index: usize) -> Result<(String, String), String> {
        if index == 0 {
            return Err("hpack index 0".into());
        }
        if index <= STATIC.len() {
            let (name, value) = STATIC[index - 1];
            return Ok((name.to_string(), value.to_string()));
        }
        let dynamic_index = index - STATIC.len() - 1;
        self.dynamic
            .get(dynamic_index)
            .cloned()
            .ok_or_else(|| format!("hpack index {index}"))
    }

    fn insert(&mut self, name: String, value: String) {
        let size = name.len() + value.len() + 32;
        if size > self.max {
            self.dynamic.clear();
            return;
        }
        self.dynamic.insert(0, (name, value));
        self.evict();
    }

    fn evict(&mut self) {
        while self.used() > self.max {
            self.dynamic.pop();
        }
    }

    fn used(&self) -> usize {
        self.dynamic
            .iter()
            .map(|(name, value)| name.len() + value.len() + 32)
            .sum()
    }
}

fn read_int(data: &[u8], prefix: u8) -> Result<(usize, &[u8]), String> {
    if data.is_empty() {
        return Err("hpack integer".into());
    }
    let mask = (1usize << prefix) - 1;
    let mut value = (data[0] as usize) & mask;
    if value < mask {
        return Ok((value, &data[1..]));
    }
    let mut rest = &data[1..];
    let mut shift = 0;
    loop {
        if rest.is_empty() || shift > 28 {
            return Err("hpack integer".into());
        }
        let byte = rest[0];
        rest = &rest[1..];
        value += ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, rest));
        }
        shift += 7;
    }
}

fn read_string(data: &[u8]) -> Result<(String, &[u8]), String> {
    if data.is_empty() {
        return Err("hpack string".into());
    }
    let huffman = data[0] & 0x80 != 0;
    let (length, rest) = read_int(data, 7)?;
    if rest.len() < length {
        return Err("hpack string".into());
    }
    let bytes = &rest[..length];
    let text = if huffman {
        decode_huffman(bytes)?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| "hpack string is not utf-8".to_string())?
    };
    Ok((text, &rest[length..]))
}

fn decode_huffman(bytes: &[u8]) -> Result<String, String> {
    let mut acc: u64 = 0;
    let mut bits = 0u8;
    let mut out = Vec::new();
    for byte in bytes {
        acc = (acc << 8) | u64::from(*byte);
        bits += 8;
        loop {
            let mut matched = None;
            for (symbol, (code, length)) in HUFFMAN.iter().enumerate() {
                if *length == 0 || bits < *length || *length > 32 {
                    continue;
                }
                let shift = bits - *length;
                let window = (acc >> shift) & ((1u64 << *length) - 1);
                if window == u64::from(*code) {
                    matched = Some((*length, symbol));
                    break;
                }
            }
            let Some((length, symbol)) = matched else {
                break;
            };
            if symbol == 256 {
                return Err("hpack eos".into());
            }
            out.push(symbol as u8);
            bits -= length;
            acc &= (1u64 << bits) - 1;
        }
    }
    if bits > 7 {
        return Err("hpack huffman padding".into());
    }
    if bits > 0 {
        let pad = acc & ((1u64 << bits) - 1);
        if pad != (1u64 << bits) - 1 {
            return Err("hpack huffman padding".into());
        }
    }
    String::from_utf8(out).map_err(|_| "hpack huffman is not utf-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_decodes_a_padded_pair_of_zeros() {
        let text = decode_huffman(&[0x00, 0x3f]).unwrap();
        assert_eq!(text, "00");
    }

    #[test]
    fn indexed_status_and_a_literal_are_decoded() {
        let mut decoder = Decoder::new();
        let mut block = vec![0x88];
        block.extend(literal("cache-control", "max-age=60"));
        let headers = decoder.decode(&block).unwrap();
        assert_eq!(headers[0], (":status".into(), "200".into()));
        assert_eq!(headers[1].0, "cache-control");
        assert_eq!(headers[1].1, "max-age=60");
    }
}
