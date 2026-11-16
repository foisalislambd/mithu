# Mithu

Mithu is a lightweight browser written in Rust, in this repository, from scratch. It is named for a rose-ringed parakeet who lived with a family and died a long time ago.

The license is MIT. Files adapted from Servo stay MPL-2.0; see [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE). Anyone can use it, read it, and build on it. The source lives at [github.com/foisalislambd/mithu](https://github.com/foisalislambd/mithu). [@foisalislambd](https://github.com/foisalislambd) maintains it.

There are two builds. They share one frame (`crates/ui`: tab strip, toolbar, omnibox, bookmarks bar, status bubble, new-tab page, Lucide icons).

The full build runs pages in [Servo](https://github.com/servo/servo), embedded through the `servo` library crate (`components/servo`, the libservo API). Mithu draws the frame and forwards navigation, history, title, load progress, tabs, and mouse, keyboard, scroll, and IME input. Servo is a git submodule at `third_party/servo` (commit `bad39019b`). Initialize it with `git submodule update --init --depth 1 third_party/servo`.

Mithu Lite (`mithu-lite`) does not link Servo or mozjs. Each tab is a system webview from the [`wry`](https://github.com/tauri-apps/wry) crate: WebView2 on Windows, WKWebView on macOS, and WebKitGTK on Linux. Android is not wired up yet. On Linux the frame and the webview share one X11 window (`GDK_BACKEND=x11`). Back, forward, and reload call wry. wry has no stop method, so Stop runs `window.stop()` in the page. Downloads are written under the user's Downloads folder when the engine reports a download. WebKitGTK also starts a web process and a network process, so the UI process is not the whole memory footprint.

Each tab is its own `WebView` on a software rendering context. The frame follows Chrome's layout: tab strip, toolbar, omnibox, bookmarks, find, zoom, and pages for history, downloads, and settings. The frame is drawn by Mithu. It does not use Google or Chrome logos. Downloads are listed in the chrome, but libservo does not expose a download callback, so files are not saved to disk yet.

The goal is a full browser that stays a thin shell around Servo. It does not add a bundled assistant or an ad system on top of that. How the frame maps onto Chromium is written in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/ROADMAP.md](docs/ROADMAP.md).

## Run

```bash
cargo run -p mithu
cargo run -p mithu -- https://example.com/
cargo run -p mithu -- --screenshot frame.png https://example.com/

# Lite does not compile Servo. On Linux it needs WebKitGTK 4.1.
cargo run -p mithu-lite
cargo run -p mithu-lite -- https://example.com/
cargo build -p mithu-lite --release
tools/package.sh lite
tools/package.sh both

cargo test --workspace
```

Linux packages for Lite: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`.

`mithu-lite --screenshot out.png URL` and `mithu-lite --hold SECONDS URL` are for a 1280×800 measurement window. `--hold` prints `load_ms` and `rss_kb` while the process stays up. A tag push runs [`.github/workflows/release.yml`](.github/workflows/release.yml), which uploads `mithu-*` and `mithu-lite-*` artifacts separately. The Lite job does not fetch the Servo submodule.

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
| `third_party/servo` | `servo` | Servo engine, linked as libservo by the full build only |
| `crates/text` | `mithu-text` | Glyphs for the frame |
| `crates/ui` | `mithu-ui` | Shared frame, Lucide icons, omnibox parsing |
| `crates/shell` | `mithu-shell` | Servo webview behind the shared frame |
| `app/mithu` | `mithu` | Full window |
| `app/mithu-lite` | `mithu-lite` | Lite window on the OS web engine |
