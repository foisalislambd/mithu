# Architecture

The full build's page engine is Servo, embedded from `third_party/servo` (`components/servo`, commit `bad39019b`). Mithu Lite (`app/mithu-lite`) does not link that engine. It hosts one `wry` webview per tab and paints the same frame from `crates/ui`. The custom DOM, CSS, JavaScript, layout, and network crates have been removed. The notes below about a from-scratch pipeline describe that earlier design.

Mithu is a small browser written in this repository. Chromium is the reference for how the pieces fit together. The checkouts used while writing this live outside the git tree, at `/home/ubuntu/scratch/`, and are not committed:

| Tree | Commit | Working tree |
| --- | --- | --- |
| `chromium/src` (`https://chromium.googlesource.com/chromium/src.git`, `--depth 1`) | `f1bd91298bf03f6f71ade7209b4f22e014889cb4` | 7.1 GB |
| `v8` (`https://chromium.googlesource.com/v8/v8.git`, `--depth 1`) | `85060e1e20544cf6909229fbfcd3d9610953c969` | 258 MB |
| `skia` (`https://skia.googlesource.com/skia.git`, `--depth 1`) | `6c39cf7f6730f7e4de69e04307df7b1cf8ab6900` | 296 MB |

A full Chromium `gclient sync` was not run. Dependencies that Chromium pulls in through `DEPS` (including the copy of V8 and Skia inside `third_party/`) are not populated in the Chromium checkout. V8 and Skia were cloned on their own so those trees are complete. Blink, `net/`, `content/`, and `chrome/browser/ui` are inside `chromium/src` and were present after the shallow clone.

## What we kept

The frame stays Mithu's. The page engine is Servo, through the `servo` crate in `third_party/servo` (`components/servo`). The custom DOM, HTML, CSS, JavaScript, layout, and network crates are gone. The window is still `winit` and `softbuffer`. Servo paints into a software framebuffer and Mithu copies that into the content rectangle under the tab strip and toolbar.

The window stays on `winit` and `softbuffer`. Chromium draws its widgets with the Views toolkit (`chrome/browser/ui/views`) and draws web content with Skia (`SkCanvas` in the Skia tree) through the compositor (`cc/`). A GPU stack (`wgpu`) or a retained widget toolkit would add a large dependency for a UI that is a handful of rectangles and glyphs. Mithu already rasterizes the page and the browser frame into one CPU buffer. That is the lightweight stand-in for Skia plus Views: one framebuffer, no GPU process.

Other existing edges stay as they are: `rustls` and `webpki-roots` for TLS, `fontdue` for glyphs, and the `image` crate (via `mithu-image`) for PNG, JPEG, GIF, and WebP. The screenshot path reuses that same `image` crate to write a PNG. `flate2` and `brotli` are the only new third-party crates. Nothing already in the workspace inflates gzip, deflate, or brotli.

## Process model

Chromium's browser process owns the window, tabs, navigation, cookies, and the cache (`chrome/browser`, `content/browser`, `net/`). A renderer process owns the document, style, layout, paint, and JavaScript (`content/renderer`, `third_party/blink`, V8). `content/browser/renderer_host/render_frame_host_impl.cc` is the browser side of a frame. `content/renderer/render_frame_impl.cc` is the renderer side.

Mithu uses the same cut, and runs both sides in one process. That is Chromium's `--single-process` mode. A second process would spend memory this browser is trying not to spend. The messages that would cross a pipe later live in `crates/shell/src/process.rs`:

- `BrowserToRenderer::Commit` is "the browser fetched this document."
- `RendererToBrowser::Committed` is "the renderer parsed it and this is the title."
- `RendererToBrowser::Navigate` is a link, form, or script asking the browser to load a new URL.

Today `Browser::install` in `crates/shell` builds the `Committed` message after the renderer crates have parsed and laid out the page. The network profile (cookie jar and HTTP cache) stays on the browser side, in `mithu-net`, and the renderer reaches it by calling `mithu_net::get`. That call is the stand-in for an IPC to the network service.

## How a page loads

Same order Chromium uses, with a much smaller implementation at each step:

1. The omnibox text becomes a URL (`mithu-url`). A hostname without a scheme is HTTPS. Other text is a DuckDuckGo query.
2. The browser process fetches it (`mithu-net`): HTTP/1.1, or HTTP/2 when TLS negotiates `h2`, redirects, `Cookie` / `Set-Cookie` (`Expires` and `SameSite`), gzip/deflate/brotli, a memory cache, and a disk cache for non-loopback `Cache-Control: max-age` responses.
3. The renderer parses HTML into a DOM (`mithu-html`, `mithu-dom`).
4. CSS is parsed and cascaded by Servo's Stylo (`mithu-style`, through the Blitz DOM). The user-agent sheet, author sheets, inline styles, and media queries all go through that cascade.
5. Boa runs classic scripts and module scripts (`mithu-js`) against that DOM.
6. Layout builds boxes with Taffy and Parley (`mithu-layout`). Paint writes pixels (`mithu-paint`) with glyphs from `mithu-text` (rustybuzz, including Bengali) and decoded images from `mithu-image`.
7. The shell composites the page under the browser frame and `softbuffer` presents it.

## Chromium component map

| Chromium | Mithu | What exists now |
| --- | --- | --- |
| `chrome/browser/ui/views/frame/browser_view.cc` and `layout/browser_view_layout.cc` | `crates/shell` frame | Tab strip, toolbar, omnibox, bookmarks bar, content, status line. `browser_view_tabbed_layout_impl.cc` stacks toolbar then bookmark bar; Mithu does the same. |
| `chrome/browser/ui/views/toolbar/toolbar_view.cc` | shell toolbar | Back, forward, reload, app menu. Icons are glyphs, not vector assets. |
| `chrome/browser/ui/views/location_bar/location_bar_view.cc` | omnibox in shell | Text entry, suggestions from bookmarks and history, star button inside the field. |
| `chrome/browser/ui/views/bookmarks/bookmark_bar_view.cc` | bookmarks bar in shell | A chip per bookmark under the toolbar. Ctrl+B still opens the list. |
| `chrome/browser/ui/color/chrome_color_id.h` | color constants in shell | Light Chrome colors: tab strip `#DEE1E6`, white toolbar, omnibox `#F1F3F4`, focus `#1A73E8`. |
| `chrome/browser/ui` new tab | `about:mithu` | Centered wordmark, a search hint, and shortcut tiles. Not Google's new tab page. |
| `content/browser` navigation | `Browser` history in shell | Per-tab history, back, forward, reload. Reload bypasses the cache. |
| `net/url_request`, `net/http/http_cache.h`, `net/spdy` | `mithu-net` | HTTP/1.1 and a one-stream HTTP/2 client, redirects, a 32-entry memory cache, and a disk cache under `MITHU_CACHE` or the temp directory. |
| `net/cookies/cookie_store.h` | `CookieJar` in `mithu-net` | Name, value, domain, path, Secure, Max-Age, Expires, SameSite. One jar for the process. |
| `net/socket/ssl_client_socket.h` | Rustls in `mithu-net` | Certificate check against the webpki root set. |
| `third_party/blink/renderer/core/html` | `mithu-html` | Tokenizer and a tree builder, including tables and the adoption agency algorithm. Not a full HTML5 parser. |
| `third_party/blink/renderer/core/dom/document.h` | `mithu-dom` | Arena DOM. Scripts can mutate it through Boa. |
| `third_party/blink/renderer/core/css` | `mithu-style` (Stylo via Blitz) | Parsing, cascade, selectors, custom properties, media queries, and `getComputedStyle`. `querySelector` still uses Mithu's selector matcher. |
| `third_party/blink/renderer/core/layout/layout_object.h` | `mithu-layout` (Taffy + Parley) | Block, flex, grid, floats, and inline line breaking. |
| `cc/` and Skia `SkCanvas` | `mithu-paint` | Software framebuffer, `0x00RRGGBB`. |
| `v8/src` | `mithu-js` (Boa) | DOM bindings, events, timers, `fetch`, modules. Boa, not V8. |
| `chrome/browser/ui/views` widget tree | `winit` + `softbuffer` in `app/mithu` | One native window. Input is forwarded into `Browser`. |

## Footprint

The design choices that keep the binary and the memory down:

- One process, one cookie jar, one memory cache (no network service process, no GPU process).
- CPU rasterization instead of Skia/GPU.
- Boa instead of V8.
- HTTP/1.1 by default, with HTTP/2 only after ALPN selects `h2`. Bodies are inflated with `flate2` (gzip, deflate) and `brotli`. Those two crates were added because nothing already in the workspace decodes `Content-Encoding`.
- Images and stylesheets are capped (16 images, stylesheets limited in size and count).

## Window

`app/mithu` creates the `winit` window and copies `Browser::render` into a `softbuffer` frame. `cargo run -p mithu -- --screenshot out.png https://example.com/` renders the same frame without opening a window, which is how the screenshot in the pull request was taken.
