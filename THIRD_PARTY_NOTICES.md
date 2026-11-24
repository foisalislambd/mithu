# Third-party notices

Mithu's own code is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
Copyright (c) 2026 Foisal Islam.

The "Mithu" name and logo are not licensed under Apache-2.0 or any other
software license.

## Servo

The Servo engine, vendored as the `third_party/servo` git submodule, remains
under the Mozilla Public License, Version 2.0. Mithu does not relicense
Servo. See https://github.com/servo/servo and https://mozilla.org/MPL/2.0/.
A short notice is also in [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE).

## Chromium

The browser frame in `crates/ui/src/chrome.rs` reproduces Chromium's desktop
UI. Layout metrics, tab geometry, and color tokens were taken from the
Chromium sources (`chrome/browser/ui/layout_constants.cc`,
`chrome/browser/ui/tabs/tab_style.cc`,
`chrome/browser/ui/views/tabs/tab_style_views.cc`,
`chrome/browser/ui/views/toolbar/toolbar_view.cc`,
`chrome/browser/ui/views/location_bar/`, `chrome/browser/ui/color/`,
`ui/color/ref_color_mixer.cc`, `ui/color/sys_color_mixer.cc`, and
`ui/views/layout/layout_provider.cc`). The vector icons under
`crates/ui/icons/chromium/` are copied from Chromium's `.icon` sources and
converted to SVG with `tools/icon2svg.py`; see `crates/ui/icons/NOTICE`.
Settings-page metrics and the Roboto webfonts under `assets/fonts/roboto/`
also come from Chromium (`ui/webui/resources/cr_elements/` and
`ui/webui/resources/roboto/`). No Google or Chrome logos, names, or product
marks are included; the product mark is Mithu's own.

The following is the license from Chromium's `LICENSE` file
(https://chromium.googlesource.com/chromium/src/+/refs/heads/main/LICENSE).

// Copyright 2015 The Chromium Authors
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//    * Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
//    * Redistributions in binary form must reproduce the above
// copyright notice, this list of conditions and the following disclaimer
// in the documentation and/or other materials provided with the
// distribution.
//    * Neither the name of Google LLC nor the names of its
// contributors may be used to endorse or promote products derived from
// this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Mithu does not include the Google or Chrome logos or those names in the
product UI. The window mark is Mithu's.

## Other third-party components

Other bundled assets and dependencies keep their own licenses, including:

- Noto Sans Bengali (`crates/text/fonts/OFL.txt`): SIL Open Font License 1.1.
- Rust crate dependencies declared in `Cargo.toml` / `Cargo.lock`, under
  each crate's own license.

## Engines (Lintel, Reed, Tercel, Gable, Purlin, Transom, Eave, Wainscot)

These engines are separate projects, each licensed under the Apache License,
Version 2.0, the same license as Mithu. Each keeps its own upstream notices in
its `THIRD_PARTY_NOTICES.md`.
