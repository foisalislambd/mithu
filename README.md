# Mithu

Mithu is a lightweight browser written in Rust, in this repository, from scratch. It is named for a rose-ringed parakeet who lived with a family and died a long time ago.

The license is MIT. Anyone can use it, read it, and build on it.

HTML, CSS, the DOM, JavaScript, layout, paint, URLs, and HTTP are implemented here. JavaScript runs in this process on the [Boa](https://github.com/boa-dev/boa) virtual machine. Scripts can change the document, and click, input, and submit events reach those scripts before the browser follows a link or sends a form. Each tab keeps its own page, history, and scroll. Ctrl+F finds text on the page, highlights every match, and scrolls the active match into view. Ctrl+Plus, Ctrl+Minus, and Ctrl+0 zoom the page and lay it out again at the new size. Ctrl+D saves or removes the current page as a bookmark, and Ctrl+B opens the list. The address bar suggests bookmarks and pages already visited. PNG, JPEG, GIF, and WebP images are decoded and painted. There is no WASM runtime and no imported browser engine. The other outside crates are the operating-system edges: a window (`winit`, `softbuffer`), TLS (`rustls`), TrueType rasterization (`fontdue`), and image decoders (`image`). Rewriting those would not make the browser stronger.

CSS covers the box model, type, class, id, attribute, and structural selectors, sibling combinators, flex layout, and relative and absolute positioning. The engine keeps growing from this code.

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
| `crates/js` | `mithu-js` | In-process JavaScript machine and DOM bindings |
| `crates/css` | `mithu-css` | Selector and declaration parser |
| `crates/style` | `mithu-style` | Cascade and computed style |
| `crates/image` | `mithu-image` | PNG, JPEG, GIF, and WebP decoding |
| `crates/text` | `mithu-text` | Font measurement and glyphs |
| `crates/layout` | `mithu-layout` | Block, inline, and flex layout |
| `crates/paint` | `mithu-paint` | Software framebuffer |
| `crates/shell` | `mithu-shell` | History, chrome, page loading |
| `app/mithu` | `mithu` | Window |
