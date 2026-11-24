# Performance: Mithu vs Chromium

This page records how Mithu's Servo edition compares with Chromium on one Linux machine, what was changed, and what is still worse. Every number below comes from `tools/bench/bench.py` on this machine. The raw runs are in [`perf-data/gpu-composite-2026-11-24.json`](perf-data/gpu-composite-2026-11-24.json). Nothing is estimated.

An earlier write-up of this page used an 8-CPU host (Chrome 156, Lite included). Those tables are not repeated here: that host is gone, and mixing the two would not be a before/after. The old files stay in [`perf-data/`](perf-data/) as `after-vs-chromium.json`, `baseline-run1.json`, and `tabs10-pressure.json`.

## Method

- **Machine:** Linux 6.12, 4 CPUs, 15 GB RAM, no `/dev/dri`. The window GL context is Mesa llvmpipe (LLVM 20.1.2), GL 4.5. Expect noise of roughly ±20% on CPU numbers.
- **Browsers:** Google Chrome 157.0.8103.50 as the Chromium reference; Mithu Servo before (unmodified `main`, release build saved before this change); Mithu Servo after (this branch). rustc 1.100.0. Lite was not rebuilt: this change does not touch it.
- **Display:** each run gets its own Xvfb at 1280×800, a fresh `HOME` and profile, and `--window-size=1280,800`. `MITHU_BENCH=1`.
- **Runs:** 3 per browser and scenario, interleaved, so network drift hits all of them. Tables show the median.
- **Process tree:** every descendant of the launched process plus anything carrying `MITHU_BENCH_TAG`. Memory is PSS from `/proc/PID/smaps_rollup`. CPU is `utime + stime`. Wakeups are voluntary plus involuntary context switches over every thread.
- **Load time:** Chromium uses `performance.timing.loadEventEnd` from launch, over the DevTools protocol. Mithu prints `bench loaded` when `LoadStatus::Complete` fires. That fires earlier relative to the work done than Chromium's load event, so Servo's load times flatter it.
- **Idle:** 10 s starting 2 s after load. **Scroll:** the English Wikipedia article on Rust, 40 wheel notches down and 40 up, 50 ms apart (xdotool). The headed after-build composites on llvmpipe (`mithu: compositing on llvmpipe … (software GL)`). There is no hardware GPU on this box, so "GPU path" here means WebRender presents into the window GL surface with no full-frame `glReadPixels`. `MITHU_SOFTWARE=1` forces the old OSMesa plus softbuffer path.
- **Chromium caveat:** a fresh profile does first-run work that inflates its CPU and memory a little.

Reproduce with:

```sh
python3 tools/bench/bench.py --runs 3 --out gpu.json \
  --browser chromium=/usr/bin/google-chrome \
  --browser servo-before=/path/to/mithu-before \
  --browser servo-after=target/release/mithu
python3 tools/bench/table.py gpu.json
```

## Results (medians of 3, 2026-11-24)

### Load time (ms, lower is better)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| example.com | 384 | 379 | 369 |
| Wikipedia main page | 455 | 394 | 398 |
| GitHub repo page | 1856 | 1010 | 740 |
| Hacker News | 851 | 639 | 641 |
| YouTube | 1403 | 420 | 417 |
| The Guardian | 3290 | 394 | 373 |
| 10 tabs | 2602 | 4079 | 3882 |
| Scroll (Rust article) | 559 | 396 | 393 |

### CPU used while loading (CPU-seconds, whole process tree)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| Blank tab | 1.67 | 0.36 | 0.45 |
| example.com | 0.34 | 0.25 | 0.26 |
| Wikipedia main page | 0.92 | 0.26 | 0.28 |
| GitHub repo page | 1.78 | 0.54 | 0.35 |
| Hacker News | 0.79 | 0.36 | 0.47 |
| YouTube | 2.17 | 0.43 | 0.45 |
| The Guardian | 4.66 | 0.26 | 0.28 |
| 10 tabs | 4.29 | 6.57 | 6.47 |
| Scroll (Rust article) | 1.05 | 0.26 | 0.25 |

### Memory after load (PSS MB, whole process tree)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| Blank tab | 537.0 | 311.0 | 331.5 |
| example.com | 487.5 | 417.7 | 419.2 |
| Wikipedia main page | 519.6 | 500.4 | 506.5 |
| GitHub repo page | 594.9 | 731.9 | 733.5 |
| Hacker News | 481.9 | 406.3 | 412.5 |
| YouTube | 725.2 | 358.0 | 364.4 |
| The Guardian | 1104.6 | 1285.0 | 997.4 |
| 10 tabs | 914.7 | 2091.7 | 1514.7 |
| Scroll (Rust article) | 589.2 | 599.1 | 613.9 |

### Idle CPU after load (% of one core, 10 s window)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| Blank tab | 3.89 | 0.0 | 0.1 |
| example.com | 4.09 | 0.0 | 0.1 |
| Wikipedia main page | 6.09 | 51.28 | 10.89 |
| GitHub repo page | 7.89 | 0.1 | 0.1 |
| Hacker News | 3.3 | 0.0 | 0.1 |
| YouTube | 9.08 | 0.0 | 0.0 |
| The Guardian | 38.41 | 183.73 | 58.33 |
| 10 tabs | 3.69 | 5.69 | 10.58 |
| Scroll (Rust article) | 6.59 | 133.01 | 20.38 |

### Idle wakeups (context switches per second, all threads)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| Blank tab | 566.6 | 19.9 | 20.0 |
| example.com | 546.4 | 1.0 | 20.0 |
| Wikipedia main page | 1080.7 | 6788.9 | 919.5 |
| GitHub repo page | 1319.6 | 1.0 | 20.0 |
| Hacker News | 562.7 | 1.0 | 20.0 |
| YouTube | 1425.8 | 1.0 | 1.0 |
| The Guardian | 9462.8 | 17418.4 | 3964.4 |
| 10 tabs | 657.9 | 267.2 | 426.8 |
| Scroll (Rust article) | 983.4 | 7686.4 | 974.7 |

### CPU for 80 wheel notches (CPU-seconds)

| Scenario | Chromium | Servo before | Servo after |
|---|---:|---:|---:|
| Scroll (Rust article) | 1.04 | 9.56 | 9.05 |

## Summary

- **Animated pages.** Wikipedia's idle CPU fell from 51% of a core to 11% (Chromium 6%). Wakeups fell from about 6,800/s to about 920/s (Chromium about 1,100/s). The Guardian fell from 184% to 58% (Chromium 38% on this run). The Rust article, once the scroll gesture is over, fell from 133% to 20% (Chromium 7%). One of the three after-runs on that article idled at 61%; the other two were 20%.
- **Scrolling.** The gesture itself only moved from 9.56 to 9.05 CPU-seconds (Chromium 1.04). On this machine the GL driver is llvmpipe, so WebRender still rasterizes every scroll frame on the CPU. Dropping the full-frame readback removed a small slice of that, not the raster. A hardware GPU uses the same present path; this box cannot show that saving.
- **10 tabs.** Settled PSS fell from 2,092 MB to 1,515 MB (Chromium 915 MB). One shared rendering context replaced a context per tab. The content processes are still the bulk, and idle CPU on the ten-tab run rose from 5.7% to 10.6%.
- **GitHub.** On this machine the after-build loaded faster and used less CPU (median 740 ms and 0.35 CPU-seconds, against 1,010 ms and 0.54 before). The before-runs were 693, 1,010 and 2,503 ms; the after-runs were 690, 740 and 766 ms. The earlier "Servo got slower on GitHub" result was on the 8-CPU host and is not what this host shows. Load CPU while a page is still parsing is capped at 10 paints per second, which is the change aimed at that.
- **Static pages.** example.com, Hacker News, YouTube and GitHub stay near 0% idle. Wakeups on several of those pages rose from about 1/s to about 20/s because a backed-off paint timer still wakes the UI thread. The CPU cost of that is about 0.1%.
- **Screenshots.** `mithu://` pages in `--screenshot` used to stay blank: `take_screenshot` waits for web fonts, the embedded fonts never satisfied that wait, and the content rectangle was left empty. Internal pages now paint through the readback path. A headed `mithu://newtab` also composites (logo and text visible).

## What this change does

The window thread owns Servo. The GL context cannot be current on two threads, and a `WebView` cannot be moved onto a different `RenderingContext` after it is created, so the window is created first and the page is painted on that thread.

| Change | Where | What it replaces |
| --- | --- | --- |
| Present the page by blitting Servo's offscreen framebuffer into the window GL surface, then draw the browser frame (tiny-skia / resvg / fontdue) as one texture. No full-frame readback on this path. | `crates/shell/src/present.rs`, `crates/composite`, `app/mithu/src/main.rs` | OSMesa framebuffer copied into softbuffer on every frame. |
| `MITHU_SOFTWARE=1`, or a failed window-context creation, keeps OSMesa plus softbuffer. `--screenshot`, `--smoke-test` and `--hold` stay headless on that path. | `app/mithu/src/main.rs`, `Browser::try_new` | — |
| One rendering context shared by every tab. Discarding a tab no longer swaps in a 64×64 context (that would resize the shared surface). | `crates/shell/src/lib.rs` (`page_context`, `discard_tab`) | One `SoftwareRenderingContext` per tab. |
| While a load is `Started` or `HeadParsed`, paint at 10 Hz. After the sampled scanlines stop changing and the user has been idle for 400 ms, back off, up to about 4 Hz. Scroll and typing cancel the backoff and skip the scanline read. | `crates/composite` (`paint_interval`), `paint_due_frame` | A fixed 60 Hz paint of every ready frame, including during parse. |
| `mithu://`, `about:` and `file:` screenshots do not wait on `take_screenshot`. A missing or flat capture is painted and read back. | `blit_page` | A font wait that expired and left the content rectangle blank. |

The browser frame is still Mithu's pixels. Unpainted pixels (value 0) are uploaded with alpha 0 so the page shows through. The chrome texture is uploaded only when the frame changes.

Earlier pacing, memory-pressure discarding, and the Lite glib loop are still in the tree. They are described in the previous version of this page; their numbers were from the other host.

## Still worse than Chromium

1. **Scroll CPU on this machine.** 9 CPU-seconds against Chromium's 1. The present path no longer reads the frame back, and llvmpipe still redraws the page for each notch. This is the gap a hardware GPU would be measured against.
2. **Animated pages are closer, not equal.** Wikipedia idle is 11% against Chromium's 6%. The Guardian is 58% against 38%. Both still run animations through a CPU rasterizer here.
3. **Ten tabs.** 1,515 MB against Chromium's 915 MB, down from 2,092 MB. Idle CPU on that run is higher than before (10.6% vs 5.7%).
4. **A few static pages wake about 20 times a second** instead of once, at about 0.1% CPU.
5. **Load times are not the same event.** Servo's complete status fires earlier than Chromium's `loadEventEnd`. One Wikipedia after-run took 1,457 ms while the other two took about 390 ms.

## Next steps

- Re-measure the scroll gesture on a machine with a hardware GL driver. The code path is the same; this host only has llvmpipe.
- Wake the UI thread on the paint deadline when a page has backed off, instead of every 50 ms.
- The remaining ten-tab PSS is mostly content processes (the cap is still four site processes, `js_mem_max` 192). Do not lower that cap without a measurement that shows the pages still work.
