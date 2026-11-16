# Mithu

Mithu is a lightweight browser written in Rust, in this repository, from scratch. It is named for a rose-ringed parakeet who lived with a family and died a long time ago.

The license is MIT. Files adapted from Servo stay MPL-2.0; see [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE). Anyone can use it, read it, and build on it. The source lives at [github.com/foisalislambd/mithu](https://github.com/foisalislambd/mithu). [@foisalislambd](https://github.com/foisalislambd) maintains it.

Pages run in [Servo](https://github.com/servo/servo), embedded through the `servo` library crate (`components/servo`, the libservo API). Mithu draws the frame and forwards navigation, history, title, load progress, tabs, and mouse, keyboard, scroll, and IME input. Servo is a git submodule at `third_party/servo` (commit `bad39019b`). Initialize it with `git submodule update --init --depth 1 third_party/servo`.

Each tab is its own `WebView` on a software rendering context. The frame follows Chrome's layout: tab strip, toolbar, omnibox, bookmarks, find, zoom, and pages for history, downloads, and settings. The frame is drawn by Mithu. It does not use Google or Chrome logos. Downloads are listed in the chrome, but libservo does not expose a download callback, so files are not saved to disk yet.

The goal is a full browser that stays a thin shell around Servo. It does not add a bundled assistant or an ad system on top of that. How the frame maps onto Chromium is written in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/ROADMAP.md](docs/ROADMAP.md).

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

`third_party/servo` is [Servo](https://github.com/servo/servo), MPL-2.0. [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE) lists it.

## Workspace

Folders are the browser layers. Package names keep a `mithu-` prefix so they stay distinct on crates.io.

| Folder | Package | Role |
| --- | --- | --- |
| `third_party/servo` | `servo` | Servo engine, linked as libservo |
| `crates/text` | `mithu-text` | Glyphs for the frame |
| `crates/shell` | `mithu-shell` | Tabs, toolbar, and the Servo webview |
| `app/mithu` | `mithu` | Window |
