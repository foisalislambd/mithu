# Measurement harness

`mithu-measure` is not the browser. It records two scores against the Mithu host. `resources.py` samples the release `mithu` binary against Chrome and Firefox.

## WPT

```
mithu-measure wpt /path/to/wpt [/path/to/resume.tsv]
```

The command serves the WPT tree on `127.0.0.1` with a static file server (not `wpt serve`) and loads each testharness page in `mithu-shell` at 800×600. `/__mithu_report.js` polyfills `Element.insertAdjacentText`, then records subtests through `add_result_callback` and `add_completion_callback` into `#__mithu_wpt`. `.any.js` and `.window.js` files are wrapped in a window document at `/__mithu_wrap__/...`. A file with `META: variant` is loaded once per variant, not also at the bare URL.

Reference files (`*-ref.html`, `*-notref.html`, `*-ref.htm`, `*-ref.xhtml`) and paths under `support/` or `resources/` are not counted.

Skipped, and not included in the pass rate:

- HTML that does not reference `testharness.js` (reftests and crash tests) — `not-testharness`
- `manual` tests
- files that contain `{{`, which need wptserve substitutions — `wptserve`
- WebSocket tests (`ws://`, `wss://`, or `WebSocket`, and `.py` helpers) — `server`
- `.any.js` / `.window.js` whose `META: global` does not include `window` — `worker-only`

The result node is written on every `add_result_callback` with phase `partial`, and again from `add_completion_callback` with phase `done`. The runner keeps pumping until the phase is `done`. A long classic script yields on the instruction budget, so the node can appear after the first subtest while later subtests are still running. Timers are stepped on the page clock: `advance_timers(0)`, then up to 200 steps of `min(next timer, 1000ms)` or 16ms while animation frames are pending. The wall cap is 90s. At the cap, or when the page is idle and completion never fired, the latest partial counts are kept. A page that never writes `#__mithu_wpt` counts as one failed subtest (`harness`). Subtests that never reach `add_result_callback` are absent from the tally. The virtual clock can leave asynchronous tests unfinished (`NOTRUN` or a partial count) that a wall-clock wpt run would complete.

A second argument is a resume TSV. Each line is `url`, `skip`, `run` (five status counts), or `harness`. The process writes `{tsv}.current` before each non-skip page. After 120 non-skip pages it exits 2 so a supervisor can resume in a fresh process. A crash (the JS host aborts on some panics, so `catch_unwind` does not always run) leaves `.current` different from the last TSV url; the supervisor appends `url harness` and continues. The in-process wall cap runs only between script turns. A native call that never returns is killed by the supervisor when one URL stays current for 180s, or when RSS exceeds 2.5 GiB, and that URL counts as one harness failure. A few early reflection files were killed at 1.5 GiB before the memory limit was raised.

`supervise-wpt.sh` is that loop. Pass rate is `pass / (pass + fail + timeout + notrun + precondition)`. Harness failures add one to `fail`.

## test262

```
mithu-measure test262 /path/to/test262 --shard=0/4
```

Every `test/**/*.js` file runs in a fresh SpiderMonkey page through `mithu-js`. `harness/assert.js` and `harness/sta.js` are included unless the test is `raw`. Tests with neither `onlyStrict` nor `noStrict` must pass in both modes (modules and `raw` run once). A negative test passes when the error name appears in the logs or the document title. Async tests pass when `$DONE()` sets `document.title` to `pass` and the logs are empty. A panic in one file counts as a failure.

## Resources

```
python3 tools/measure/resources.py
```

`/usr/bin/time` is not required. The script samples `/proc/PID/status` (`VmRSS`, `Threads`) and `/proc/PID/stat` (`utime` + `stime`, fields after `comm`) every 100ms and sums them across the process tree. Shared pages are counted once per process. `SC_CLK_TCK` converts ticks to seconds.

Mithu is `target/release/mithu --hold 12 URL TABS` at 1280×800. `loaded` is printed after navigation and the screenshot quiet-pump. The process stays up 12s. Steady RSS is the sample closest to 10s after `loaded`. CPU-to-load is the tree's user+sys at the first sample at or after `loaded`. Wall load is process start to `loaded`. `TABS` is the total tab count; extra tabs are `about:blank`.

Chrome, when present, uses a separate `--headless=new --dump-dom` process for wall time and CPU-to-load (cold start included, no virtual-time budget). Chrome writes the DOM and keeps running, so load time is the moment stdout contains `</html>`, and that process is then killed. A second process with remote debugging is the RSS sample; its steady point is `start + dump_wall + 10s`. Five blank tabs use `PUT /json/new`. Chrome 157 rejects GET on that route.

Firefox is not the apt package (that package is a snap stub). The run uses the Mozilla linux64 build at `/tmp/firefox/firefox` when `firefox` is not on `PATH`. The screenshot profile directory is created before launch. Wall time and CPU-to-load come from a separate `--headless --screenshot` process, which exits after painting the page. The held process is headless at 1280×800; its steady point uses that screenshot duration the same way Chrome uses the dump. Five blank tabs are the first `about:blank` plus four `--new-tab about:blank` arguments on that process. One sample per case. Screenshot load and dump-dom load are different browser events; the tables name which one each column used.
