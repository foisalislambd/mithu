# Roadmap

Mithu already loads a page: URL, HTTP, HTML, CSS, DOM, Boa, layout, and paint, plus tabs, history, find, zoom, and bookmarks. The notes below are what that covers, what this round added, and what should come next. Chromium paths refer to the shallow tree described in [ARCHITECTURE.md](ARCHITECTURE.md).

## Done

- Window, tab strip, toolbar, per-tab history, find in page, zoom, bookmarks.
- HTTP/1.1 and HTTPS (Rustls), redirects, chunked bodies.
- HTML tree builder, CSS subset, block/inline/flex layout, software paint, images.
- JavaScript on Boa: DOM, events, timers, `fetch`, modules. This stays on Boa.

## This round

- Chrome-like frame, following `BrowserView` and `browser_view_tabbed_layout_impl.cc`: gray tab strip, white toolbar, pill omnibox, back / forward / reload, star, three-dot menu, bookmarks bar, and a new-tab page with shortcut tiles.
- Profile cookie jar (`net/cookies/cookie_store.h` is the reference) and an in-memory HTTP cache (`net/http/http_cache.h` is the reference). Reload bypasses the cache.
- An explicit browser/renderer message enum in `crates/shell/src/process.rs`. Both sides still run in one process.
- Headless screenshot: `cargo run -p mithu -- --screenshot out.png <url>`.
- Architecture map in `docs/ARCHITECTURE.md`.

## Next, in the order they make real pages look right

1. **More of the pages people open.** CSS grid, floats, `position: sticky`, and a larger slice of selectors and properties. Blink's `style_resolver` and `layout_object` are the reference. Simple articles should reflow closer to Chromium before any new subsystem.
2. **HTTP that real sites require.** `Content-Encoding: gzip`, HTTP/2, and a disk cache with validators (`ETag`, `Last-Modified`). The memory cache only stores `Cache-Control: max-age` and skips `Set-Cookie`.
3. **Cookies, the rest of the jar.** `Expires`, `SameSite`, and partitioning. Scripts still cannot read the jar, which matches `HttpOnly` being invisible to Boa.
4. **A real renderer boundary, still optional.** The messages in `process.rs` can move to a pipe if a crash in Boa should not take down the window. Measure memory before doing it. Site isolation (`docs/process_model_and_site_isolation.md` in the Chromium tree) is not the goal for a lightweight browser.
5. **Text and paint.** Shaping for more scripts, and a clip stack that matches overflow. Skia's `SkCanvas` is the reference for paint operations we do not have yet (shadows, gradients, filters). Stay on the CPU buffer until a page cannot be drawn without a GPU.
6. **JavaScript, still on Boa.** More DOM (forms, `classList` is already there; add the pieces pages actually call), and a tighter event loop. Read `v8/src` when the question is how an embedder should behave. Do not replace Boa with V8.

## Explicitly out of scope until the list above moves

WebGPU, a bundled assistant, an ad system, extension APIs, and a full port of Views. The window will keep using `winit` and `softbuffer`.
