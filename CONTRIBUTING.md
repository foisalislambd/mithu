# Contributing to Mithu

Mithu is open source under the BSD 3-Clause license. Issues and pull requests are welcome at [github.com/foisalislambd/mithu](https://github.com/foisalislambd/mithu).

## What to send

A useful change is a focused fix or a small, tested piece of the browser: HTML, CSS, the DOM, JavaScript bindings, layout, paint, networking, or the window. Write product text and documentation in English.

Please open an issue before a large rewrite. The maintainer is [@foisalislambd](https://github.com/foisalislambd).

## Build

Rust stable is pinned in `rust-toolchain.toml`.

```bash
cargo test --workspace
cargo run -p mithu
cargo run -p mithu -- https://example.com/
```

`cargo fmt --all` before a pull request. Workspace crates forbid `unsafe` code. Dependencies may use it. Do not weaken TLS hostname checks.

The `chromium/` directory is a local reference tree. It is gitignored. Do not add it to a commit, and do not copy its source into this repository.

## Pull requests

Describe the behavior the change adds or fixes, and how you tested it. Keep the diff limited to that change. Screenshots help when the window or page layout changes.
