# Changelog

Mithu follows the version in the workspace `Cargo.toml`. Dates are the day the release is tagged.

## Unreleased

Chrome-like frame (tab strip, omnibox, bookmarks bar, app menu, new tab page, tab drag, history, downloads). The network profile stores cookies (`Expires`, `SameSite`), inflates gzip, deflate, and brotli, speaks HTTP/2 when TLS negotiates it, and reuses fresh `Cache-Control: max-age` responses from memory and disk. Layout covers floats, flex shrink, and a basic grid. JavaScript remains on Boa. See `docs/ARCHITECTURE.md`.

## 0.1.0

First public tree of the Mithu browser. The window, tabs, navigation, HTTP, HTML, CSS, layout, paint, images, and in-process JavaScript are in this repository and still growing.
