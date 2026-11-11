# Changelog

Mithu follows the version in the workspace `Cargo.toml`. Dates are the day the release is tagged.

## Unreleased

Chrome Refresh frame: tab strip, caption buttons, toolbar, omnibox chip, bookmarks bar, app menu, new tab page, tab drag, history, downloads, and settings. Toolbar glyphs are Chromium's BSD vector icons (`crates/shell/icons/NOTICE`). The network profile stores cookies (`Expires`, `SameSite`), inflates gzip, deflate, and brotli, speaks HTTP/2 when TLS negotiates it, and reuses fresh `Cache-Control: max-age` responses from memory and disk. Layout covers floats, flex shrink, and a basic grid. CSS keeps rules inside `@layer` and `@supports`, unescapes selector identifiers, and applies logical `margin`/`padding`/`inset` plus `:root` custom properties. The `[hidden]` attribute hides a subtree. HTML comments stay in the tree, and comment nodes expose `data`, `nodeType`, and sibling links so a page can move nodes around them. `self`, `performance.now`, and `requestAnimationFrame` are available to classic scripts. JavaScript remains on Boa. See `docs/ARCHITECTURE.md`.

## 0.1.0

First public tree of the Mithu browser. The window, tabs, navigation, HTTP, HTML, CSS, layout, paint, images, and in-process JavaScript are in this repository and still growing.
