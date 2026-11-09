# Mithu

Mithu is a lightweight browser written in Rust, in this repository, from scratch. It is named for a rose-ringed parakeet who lived with a family and died a long time ago.

The license is MIT. Anyone can use it, read it, and build on it.

HTML, CSS, the DOM, layout, paint, URLs, and HTTP are implemented here. There is no WASM runtime and no imported browser engine. The only outside crates are the operating-system edges: a window (`winit`, `softbuffer`), TLS (`rustls`), and TrueType rasterization (`fontdue`). Rewriting those would not make the browser stronger.

The goal is a full browser that stays small: pages, navigation, network, layout, and the web platform people actually use. It does not add a bundled assistant or an ad system on top of that.

## Run

```bash
cargo run -p mithu
cargo run -p mithu -- https://example.com/
cargo test --workspace
```

Type a URL in the address bar and press Enter. A hostname without a scheme opens as HTTPS. Other text is sent to DuckDuckGo. Alt+Left and Alt+Right walk history. Ctrl+L focuses the address bar. Ctrl+R reloads.

## Workspace

Folders are the browser layers. Package names keep a `mithu-` prefix so they stay distinct on crates.io.

| Folder | Package | Role |
| --- | --- | --- |
| `crates/url` | `mithu-url` | URL parser and relative resolution |
| `crates/net` | `mithu-net` | HTTP/1.1, chunked bodies, redirects, TLS |
| `crates/dom` | `mithu-dom` | Document tree |
| `crates/html` | `mithu-html` | Tokenizer and tree builder |
| `crates/css` | `mithu-css` | Selector and declaration parser |
| `crates/style` | `mithu-style` | Cascade and computed style |
| `crates/text` | `mithu-text` | Font measurement and glyphs |
| `crates/layout` | `mithu-layout` | Block and inline layout |
| `crates/paint` | `mithu-paint` | Software framebuffer |
| `crates/shell` | `mithu-shell` | History, chrome, page loading |
| `app/mithu` | `mithu` | Window |
