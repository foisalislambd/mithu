# Measuring engine progress (short version)

Each engine repo (Lintel, Reed, Tercel, Gable, Purlin, Transom, Eave, Wainscot) has a full `MEASURING.md`. This is the short version for Mithu contributors and agents.

We do not port Chromium line by line. Chromium (and V8, Skia, PDFium) is the reference design; each engine is our own Rust code with the same behavior, using good crates where they exist. So progress is measured by behavior, with two numbers per engine:

1. **Headline: upstream conformance.** Upstream test cases (`TEST` / `TEST_F` / `TEST_P`) reproduced as Rust `#[test]`s and passing, divided by **all** upstream test cases in the engine's scoped directories. Unported tests stay in the total as 0. Show `passed / total (x.x%)` and the upstream commit.
2. **Real-world / spec suite.** Lintel: Test262. Reed: WPT reftests. Wainscot: PDFium pixel corpus. Gable: Skia GM pixel tests. Tercel, Eave, Transom, Purlin: WPT for their area once Mithu hosts them; until then "not yet measured".

Rules:

- A crate (`rustls`, `wgpu`, `vello`, `redb`, ...) counts as done only when the matching upstream tests pass through our API.
- No subjective 0-1 marks, capability scores, weighting, half credit for "partial", counting our own tests, or rounding up. The old headline percentages are retired.
- Every number must come from a command listed in the engine's `MEASURING.md`, and every PR that changes progress updates the table at the top of that engine's `GAPS.md` with before -> after numbers from an actual run.
- If a total cannot be computed reliably, write "not yet measured".
