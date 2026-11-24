# Engine seams

Mithu ships two page engines today: Servo (`app/mithu`, through `crates/shell`) and the OS webview (`app/mithu-lite`, through `wry`). Our own engines (Lintel, Reed, Eave and the rest) are meant to replace parts of those later. This page lists where an engine plugs in, so a replacement only has to satisfy these seams and keeps everything around them, including the performance work in [PERF.md](PERF.md).

## What is engine-agnostic already

| Piece | Where | Depends on an engine? |
| --- | --- | --- |
| Browser frame (tab strip, toolbar, omnibox, menus) | `crates/ui/src/chrome.rs` (`chrome::paint`, `chrome::hit`) | No. Takes a `chrome::State` and a `u32` pixel buffer. |
| Settings, history, bookmarks, internal pages | `crates/ui/src/pages.rs` | No. |
| Wheel scroll animation | `crates/ui/src/scroll.rs` (`ScrollAnimator`) | No. Produces per-frame pixel deltas; the engine only has to accept "scroll by N px at point P". |
| Memory pressure and discard policy | `crates/ui/src/memory.rs` (`Monitor`, `pick_discard`) | No. The engine only has to be able to drop a page and reload it from its URL. |
| Benchmark hooks | `crates/host` (`bench_enabled`, `bench_report_loaded`) | No. The engine reports "load complete" once per tab. |
| Benchmark harness | `tools/bench/bench.py` | No. Measures any process tree. |

## The seams

### 1. Page host (one per tab)

Servo: `crates/shell/src/lib.rs`, `struct Tab { webview: servo::WebView, context: Rc<dyn RenderingContext>, .. }`. Every tab in a window shares one context: an offscreen framebuffer of the window GL context when compositing, otherwise one OSMesa `SoftwareRenderingContext`.
Lite: `app/mithu-lite/src/main.rs`, `struct Tab { id, webview: wry::WebView }`.

A replacement engine provides, per tab:

| Operation | Servo call today | Lite call today |
| --- | --- | --- |
| Load a URL | `WebView::load(url)` | `WebView::load_url` |
| Back / forward / reload / stop | `go_back`, `go_forward`, `reload` | `go_back`, `go_forward`, `reload`; Stop runs `window.stop()` in the page |
| Resize the viewport | `WebView::resize` | `WebView::set_bounds` |
| Show / hide (background throttling) | `show` / `hide` | `set_visible` |
| Load state, URL, title, favicon | `load_status`, `url`, `page_title`, `favicon` | `on_page_load_handler`, title handler |
| Input: pointer, wheel, keys, IME | `notify_input_event`, `notify_scroll_event` | handled by the OS webview |
| "A new frame is ready" | `WebViewDelegate::notify_new_frame_ready` | n/a (the OS composites) |
| Present the frame | `OffscreenRenderingContext::render_to_parent_callback` blits into the window; `read_to_image` is the software and screenshot fallback | the OS composites |
| Drop the page (discard) | drop the `WebView` | drop the `wry::WebView` |

### 2. Event-loop wake

The engine must wake the UI loop when it has work instead of being polled. Servo does this through `EventLoopWaker` (`ServoWake` in `crates/shell`), which pokes `EngineWake`. The winit thread runs Servo: the GL context is thread-affine, so the page and the window have to be painted on the same thread. `EngineWake` posts a winit user event when Servo has work. Lite's WebKitGTK sources are glib sources, and `run_with_glib` blocks in one glib poll that also watches winit's fd. A new engine either exposes an fd/glib source or calls a waker; it should never require a fixed-rate poll.

### 3. Frame hand-off

Headed Servo presents on the GPU. `crates/shell/src/present.rs` blits the page framebuffer into the content rectangle and draws the browser frame from `crates/composite` (one RGBA texture, uploaded only when the chrome changed). The software fallback and `--screenshot` still go through `Browser::render`, which returns a `0RGB` buffer for the whole window and, on the fallback, copies it into `softbuffer` only when `content_revision` changed. A replacement engine can satisfy the same seam with its own surface: implement the present path, and keep `Browser::render` for the headless tools. The chrome pixels stay engine-agnostic (`crates/ui`).

### 4. Network, storage, JavaScript

These live inside the engine today (Servo's `net`, `storage`, SpiderMonkey; WebKit's network process). Eave (storage) and the other engines plug in below the page host, so the seams above do not change when they arrive.
