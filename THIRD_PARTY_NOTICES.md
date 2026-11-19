# Third-party notices

Mithu's own code is licensed under the BSD 3-Clause License. See [LICENSE](LICENSE).
Copyright (c) 2026 Foisal Islam.

The "Mithu" name and logo are not licensed under BSD-3-Clause or any other
software license.

## Servo

The Servo engine, vendored as the `third_party/servo` git submodule, remains
under the Mozilla Public License, Version 2.0. Mithu does not relicense
Servo. See https://github.com/servo/servo and https://mozilla.org/MPL/2.0/.
A short notice is also in [THIRD_PARTY/NOTICE](THIRD_PARTY/NOTICE).

## Chromium

Selected layout metrics, color tokens, filled Material icon paths under
`crates/ui/icons/`, and the Roboto webfonts under
`assets/fonts/roboto/` come from the Chromium project
(`ui/webui/resources/cr_elements/cr_shared_vars.css`,
`ui/webui/resources/cr_elements/cr_nav_menu_item_style_lit.css`,
`ui/color/ref_color_mixer.cc`, `ui/color/sys_color_mixer.cc`, and
`ui/webui/resources/roboto/`). Additional Chromium vector icons are noted in
`crates/ui/icons/NOTICE`.

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

- Lucide icons (`THIRD_PARTY/lucide-LICENSE`): ISC, with portions derived
  from Feather (MIT).
- Noto Sans Bengali (`crates/text/fonts/OFL.txt`): SIL Open Font License 1.1.
- Rust crate dependencies declared in `Cargo.toml` / `Cargo.lock`, under
  each crate's own license.

## Engines (Lintel, Reed, Tercel, Gable)

Lintel, Reed, Tercel, and Gable are separate projects, each dual-licensed
under AGPL-3.0-only and a commercial license. A Mithu build that links any
of those engines is subject to AGPL-3.0 unless a commercial license is
obtained from Foisal Islam.
