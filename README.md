# Mithu

Mithu is a lightweight browser written in Rust, in this repository, from scratch. It is named for a rose-ringed parakeet who lived with a family and died a long time ago.

The license is MIT. Files adapted from Servo stay MPL-2.0; see [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE). Anyone can use it, read it, and build on it. The source lives at [github.com/foisalislambd/mithu](https://github.com/foisalislambd/mithu). [@foisalislambd](https://github.com/foisalislambd) maintains it.

HTML, CSS, the DOM, JavaScript, layout, paint, URLs, and HTTP are implemented here. The HTML parser builds the implied document, tables, and `select` lists, and it repairs mis-nested formatting tags. `white-space: nowrap` keeps a line from wrapping. JavaScript runs in this process on the [Boa](https://github.com/boa-dev/boa) virtual machine, including module scripts. Scripts can change the document, listen for events, set timers, call `fetch`, and assign `location.href`. Click, input, and submit events reach those scripts before the browser follows a link or sends a form. Each tab keeps its own page, history, and scroll. Ctrl+F finds text on the page, highlights every match, and scrolls the active match into view. Ctrl+Plus, Ctrl+Minus, and Ctrl+0 zoom the page and lay it out again at the new size. Ctrl+D saves or removes the current page as a bookmark, and Ctrl+B opens the list. The address bar suggests bookmarks and pages already visited. A right click opens a menu to follow a link, open it in a new tab, go back, reload, or bookmark the page. PNG, JPEG, GIF, and WebP images are decoded and painted. There is no WASM runtime and no imported browser engine. The other outside crates are the operating-system edges: a window (`winit`, `softbuffer`), TLS (`rustls`), TrueType rasterization (`fontdue`), image decoders (`image`), and content-encoding (`flate2`, `brotli`). Rewriting those would not make the browser stronger. JavaScript stays on Boa.

CSS and layout go through Servo's Stylo and the Taffy and Parley crates (the same path Dioxus Blitz uses). That covers the cascade, selectors, custom properties, `calc()`, media queries, `::before` and `::after`, flex, grid, floats, and inline line breaking. `getComputedStyle` reads the Stylo values. JavaScript stays on Boa. `iframe` and `noscript` keep their contents as text. Scripts can use `classList`, element children, and `removeChild`.

The goal is a full browser that stays small: pages, navigation, network, layout, and the web platform people actually use. It does not add a bundled assistant or an ad system on top of that.

The frame follows Chrome's layout: tab strip, toolbar, omnibox, bookmarks bar, and a new-tab page. JavaScript stays on Boa. How that maps onto Chromium, and what is intentionally not ported, is written in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/ROADMAP.md](docs/ROADMAP.md).

## Run

```bash
cargo run -p mithu
cargo run -p mithu -- https://example.com/
cargo run -p mithu -- --screenshot frame.png https://example.com/
cargo test --workspace
```

Type a URL in the address bar and press Enter. A hostname without a scheme opens as HTTPS. Other text is sent to DuckDuckGo. Alt+Left and Alt+Right walk history. Ctrl+L focuses the address bar. Ctrl+R reloads.

Rust stable is pinned in `rust-toolchain.toml`. `cargo test --workspace` runs the suite.

## Contributing

Issues and pull requests go to [github.com/foisalislambd/mithu](https://github.com/foisalislambd/mithu). Read [CONTRIBUTING.md](CONTRIBUTING.md) before sending a change, and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for how people work together here. Report a security problem through [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE). Copyright 2026 Mithu contributors.

`crates/net/src/cors.rs` and `crates/js/src/custom_element.rs` are adapted from [Servo](https://github.com/servo/servo) and are MPL-2.0. The headers on those files name the Servo sources. [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE) lists them.

## Workspace

Folders are the browser layers. Package names keep a `mithu-` prefix so they stay distinct on crates.io.

| Folder | Package | Role |
| --- | --- | --- |
| `crates/url` | `mithu-url` | URL parser and relative resolution |
| `crates/net` | `mithu-net` | HTTP/1.1 and HTTP/2, gzip/brotli, cookies, memory and disk cache |
| `crates/dom` | `mithu-dom` | Document tree |
| `crates/html` | `mithu-html` | Tokenizer and tree builder |
| `crates/js` | `mithu-js` | In-process JavaScript machine and DOM bindings |
| `crates/css` | `mithu-css` | Selector and declaration parser |
| `crates/style` | `mithu-style` | Cascade and computed style |
| `crates/image` | `mithu-image` | PNG, JPEG, GIF, and WebP decoding |
| `crates/text` | `mithu-text` | Font measurement and glyphs |
| `crates/layout` | `mithu-layout` | Taffy block, flex, grid, and float layout, with Parley line breaking |
| `crates/paint` | `mithu-paint` | Software framebuffer |
| `crates/shell` | `mithu-shell` | History, chrome, page loading |
| `app/mithu` | `mithu` | Window |
