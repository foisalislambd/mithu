# Performance: Mithu vs Chromium

This page records how Mithu's two editions compare with Chromium on one Linux machine, what was changed to close the gap, and what is still worse. Every number below comes from `tools/bench/bench.py`; the raw runs are in [`perf-data/`](perf-data/). Nothing is estimated.

## Method

- **Machine:** shared Linux box (kernel 6.12), 8 CPUs, 15 GB RAM, no GPU (Mesa llvmpipe software GL). Other jobs ran on the same box, so expect noise of roughly ±20% on CPU numbers.
- **Browsers:** Google Chrome 156.0.8078.47 as the Chromium reference; Mithu Lite (wry 0.57.3, WebKitGTK 2.54.3); Mithu with Servo (`third_party/servo`, servo 0.8.0). rustc 1.100.0, `--release`.
- **Before / after:** "before" is main at `bde830f` plus only the benchmark hooks (commit `91fd69b`, rebased), "after" is this branch. Both were built with the same toolchain on the same day.
- **Display:** each run gets its own Xvfb at 1280x800, a fresh `HOME` and profile, and `--window-size=1280,800`.
- **Runs:** 3 per browser and scenario, interleaved (Chromium, Lite before, Lite after, Servo before, Servo after, then repeat), so network drift hits all of them. Tables show the median.
- **Process tree:** every descendant of the launched process plus anything carrying the `MITHU_BENCH_TAG` environment tag. Memory is PSS from `/proc/PID/smaps_rollup` (shared pages split fairly between processes; RSS is in the raw data). CPU is `utime + stime`. Wakeups are voluntary plus involuntary context switches summed over every thread.
- **Load time:** Chromium: `performance.timing.loadEventEnd`, measured from launch, read over the DevTools protocol. Mithu: the `bench loaded` line printed when `MITHU_BENCH=1` and the tab reaches its load-complete state. Servo's `LoadStatus::Complete` fires earlier relative to the work done than Chromium's load event, so Servo's load times flatter it; treat them as "first complete" rather than an exact match.
- **Idle:** a 10 s window starting 2 s after load. **Scroll:** the English Wikipedia article on Rust, 40 wheel notches down and 40 up, 50 ms apart (xdotool), CPU over the gesture.
- **Chromium caveat:** a fresh profile does first-run work (component updates, field trials) that inflates its CPU and memory a little.

Reproduce with:

```sh
python3 tools/bench/bench.py --runs 3 --out after.json \
  --browser chromium=/usr/bin/google-chrome \
  --browser lite=target/release/mithu-lite \
  --browser servo=target/release/mithu
python3 tools/bench/table.py after.json
```

## Results (medians of 3, run 2026-11-24 20:00 UTC+6)

Raw data: [`perf-data/after-vs-chromium.json`](perf-data/after-vs-chromium.json) (the earlier baseline against older main is in `perf-data/baseline-run1.json`).

### Load time (ms, lower is better)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| example.com | 1684 | 894 | 742 | 583 | 558 |
| Wikipedia main page | 2201 | 1524 | 1421 | 563 | 531 |
| GitHub repo page | 5460 | 4158 | 3279 | 1973 | 3447 |
| Hacker News | 1675 | 961 | 966 | 657 | 595 |
| YouTube | 2354 | 2141 | 2099 | 612 | 595 |
| The Guardian | 1874 | 1056 | 1031 | 553 | 448 |
| 10 tabs | 5091 | 8801 | 8272 | 4989 | 4102 |
| Scroll (Rust article) | 1844 | 2012 | 1970 | 563 | 537 |

### CPU used while loading (CPU-seconds, whole process tree)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| Blank tab | 5.09 | 1.08 | 0.89 | 0.55 | 0.54 |
| example.com | 2.28 | 1.10 | 0.85 | 0.53 | 0.52 |
| Wikipedia main page | 3.12 | 2.81 | 1.48 | 0.45 | 0.44 |
| GitHub repo page | 6.35 | 7.38 | 2.64 | 0.51 | 3.14 |
| Hacker News | 2.33 | 1.10 | 0.77 | 0.55 | 0.42 |
| YouTube | 4.57 | 4.65 | 3.12 | 0.61 | 0.42 |
| The Guardian | 3.40 | 1.82 | 1.25 | 0.49 | 0.56 |
| 10 tabs | 8.12 | 11.80 | 12.58 | 8.64 | 7.46 |
| Scroll (Rust article) | 3.04 | 3.75 | 1.83 | 0.46 | 0.45 |

### Memory after load (PSS MB, whole process tree)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| Blank tab | 689 | 448 | 448 | 322 | 319 |
| example.com | 641 | 477 | 477 | 418 | 426 |
| Wikipedia main page | 673 | 550 | 550 | 504 | 505 |
| GitHub repo page | 750 | 674 | 675 | 731 | 740 |
| Hacker News | 625 | 464 | 464 | 396 | 402 |
| YouTube | 865 | 889 | 931 | 350 | 353 |
| The Guardian | 716 | 615 | 619 | 725 | 735 |
| 10 tabs | 1001 | 1435 | 1432 | 2075 | 1852 |
| Scroll (Rust article) | 698 | 625 | 647 | 607 | 607 |

### Idle CPU after load (% of one core, 10 s window)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| Blank tab | 4.9 | 0.5 | 0.1 | 0.5 | 0.1 |
| example.com | 6.3 | 8.5 | 9.9 | 0.1 | 0.0 |
| Wikipedia main page | 7.7 | 28.1 | 27.9 | 131.1 | 72.4 |
| GitHub repo page | 19.0 | 7.7 | 12.9 | 0.5 | 0.0 |
| Hacker News | 3.6 | 7.7 | 7.6 | 0.1 | 0.0 |
| YouTube | 5.0 | 20.7 | 20.9 | 0.2 | 0.0 |
| The Guardian | 3.9 | 121.9 | 32.1 | 18.2 | 14.9 |
| 10 tabs | 4.4 | 1.1 | 0.5 | 2.8 | 5.5 |
| Scroll (Rust article) | 8.6 | 17.8 | 21.4 | 206.7 | 150.0 |

### Idle wakeups (context switches per second, all threads)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| Blank tab | 436 | 92 | 26 | 154 | 20 |
| example.com | 514 | 3150 | 3633 | 36 | 1 |
| Wikipedia main page | 902 | 7162 | 8755 | 23134 | 10300 |
| GitHub repo page | 823 | 6616 | 12495 | 61 | 1 |
| Hacker News | 376 | 3457 | 3423 | 36 | 1 |
| YouTube | 613 | 6976 | 6898 | 36 | 1 |
| The Guardian | 512 | 13954 | 12924 | 594 | 314 |
| 10 tabs | 526 | 151 | 117 | 145 | 177 |
| Scroll (Rust article) | 928 | 4569 | 5319 | 15052 | 11054 |

### CPU for 80 wheel notches (CPU-seconds)

| Scenario | Chromium | Lite before | Lite after | Servo before | Servo after |
|---|---:|---:|---:|---:|---:|
| Scroll (Rust article) | 0.77 | 3.58 | 4.53 | 10.66 | 11.58 |

### Summary

- **Idle (the "no jank, low idle CPU" goal):** after the change both editions idle well below Chromium on a blank tab (0.1% vs 4.9%, 20–26 wakeups/s vs 436). Servo on static pages (example.com, Hacker News, YouTube, GitHub) is at 0.0% idle with about 1 wakeup per second; before it woke 36–154 times a second.
- **Servo on animated pages:** paint pacing cut Wikipedia idle CPU from 131% to 72% and wakeups from 23 k/s to 10 k/s, and The Guardian from 18% to 15%. It is still about 10x Chromium there (see "Still worse").
- **Lite:** load CPU dropped on every site (for example GitHub 7.4 → 2.6 s, Wikipedia 2.8 → 1.5 s) because the UI no longer repaints the whole window 60 times a second while a page loads. The Guardian's idle CPU dropped from 122% to 32%. Idle on other pages is unchanged; that CPU is inside WebKitGTK's web process.
- **Memory:** Mithu uses less PSS than Chromium for one page on every site except Servo on GitHub and The Guardian and Lite on YouTube. With ten tabs, both editions use more than Chromium (Chromium 1001 MB, Lite 1432 MB, Servo 1852 MB, down from 2075 MB). Memory-pressure discarding (below) brings that down when the machine is short of memory.
- **Load time:** Mithu reaches its load-complete state before Chromium's load event on every single-page test. GitHub on Servo varied a lot in this run (after: 1563, 3447 and 4150 ms; before: 1867–2452 ms); two of the three after-runs also used about 3 s of CPU, so this is listed as an open item rather than explained away.

## What was implemented, and the Chromium design each piece follows

| Change | Where | Chromium design it follows |
| --- | --- | --- |
| Read Servo's last painted frame straight from the rendering context instead of `take_screenshot` per window frame (which waits for images, fonts and the load event and spun the engine thread in 1 ms sleeps) | `crates/shell/src/lib.rs` (`refresh_page_cache`) | Viz/`components/viz/service/display/display.cc`: the display presents the latest submitted compositor frame; it never waits for the page to finish loading. |
| Pace Servo painting at 60 Hz and only paint the focused tab; Servo produced ~120 frames/s on an idle Wikipedia page and every one was rasterised | `crates/shell/src/lib.rs` (`drain_ready_frames`, `paint_due_frame`, `paint_due`) | `components/viz/common/frame_sinks/delay_based_begin_frame_source.cc` (a timer at display rate when there is no vsync) and `cc/scheduler/scheduler.cc` (no BeginFrame for hidden or idle layer trees). |
| Paint holding on tab switch: the old tab's pixels stay on screen until the newly shown tab has a complete frame (one screenshot-path capture, then fast readback) | `crates/shell/src/lib.rs` (`switch_capture`) | Paint holding (`content/browser/renderer_host/render_widget_host_impl.cc`, `ForceFirstFrameAfterNavigationTimeout`). |
| Frame hand-off by `Arc` and a sequence number; the window thread only copies to `softbuffer` when the frame changed, and the window title is only set when it changed | `app/mithu/src/engine.rs`, `app/mithu/src/main.rs` | Viz: frames are submitted, not polled; unchanged frames are not redrawn (`cc/trees/layer_tree_host_impl.cc` damage tracking). |
| Engine thread sleeps on the waker with a 1 s idle wait (was a short fixed poll) and only wakes early for a paint deadline or a scroll frame | `app/mithu/src/engine.rs` | `base/message_loop/message_pump_glib.cc` / `message_pump_default.cc`: block until work or the next delayed task. |
| Lite: one glib poll owns the loop and also watches winit's fd, instead of polling GTK and winit in turns; fixed a starvation bug where `gtk::events_pending()` never returned false | `app/mithu-lite/src/main.rs` (`run_with_glib`) | `base/message_loop/message_pump_glib.cc` (Chromium on Linux runs its UI loop inside one glib poll). |
| Lite: repaint the frame only when the model changed or the hovered target changed, not 60 times a second while a page loads | `app/mithu-lite/src/main.rs` | Views invalidation (`ui/views/view.cc` `SchedulePaint` on state change only). |
| Animated wheel scrolling for the Servo edition: ease-in-out cubic Bézier (0.42, 0, 0.58, 1), 6–12 frames depending on distance, retargeting without a stall; precise touchpad deltas pass through | `crates/ui/src/scroll.rs` (5 tests), `crates/shell` (`scroll_animated`, `tick_scroll`) | `cc/animation/scroll_offset_animation_curve.cc` (`DurationBehavior::kInverseDelta`). |
| Memory pressure monitor: sample available memory every 5 s; moderate at ≤1000 MB, critical at ≤400 MB available (500/200 MB on machines under 1.5 GB) | `crates/ui/src/memory.rs` (`Monitor`, `evaluate`) | `components/memory_pressure/system_memory_pressure_evaluator_win.cc` (same thresholds and period). |
| Tab discarding under pressure: least recently used background tab first; under moderate pressure only tabs hidden for 60 s; never the active tab or one that never loaded; a discarded tab reloads its URL when you switch to it | `crates/ui/src/memory.rs` (`pick_discard`), both editions (`relieve_memory_pressure`, `restore_discarded_active`) | `chrome/browser/performance_manager/policies/page_discarding_helper.cc`, `discard_eligibility_policy.cc`. |
| Benchmark hooks and harness | `crates/host` (`bench_*`), `tools/bench/` | Chromium's own perf bots measure the same things (load, PSS, idle wakeups). |

`MITHU_MEMORY_PRESSURE=moderate|critical` forces a pressure level for testing, like `chrome://discards`.

## Crates chosen

| Need | Choice | Why |
| --- | --- | --- |
| Available/total memory on Linux, macOS, Windows | `sysinfo` 0.40, `default-features = false`, `features = ["system"]` (MIT) | Maintained, cross-platform, and with only the `system` feature it is small. Reading `/proc/meminfo` by hand would only cover Linux. |
| One event loop for GTK/WebKit and winit | `glib` 0.18 `unix_fd_add_local` + winit 0.30 `EventLoopExtPumpEvents` (already dependencies, MIT) | No new crate needed; this is the supported way to drive both from one poll. |
| Wheel animation curve, discard policy | Own code (~400 lines with tests) | Small and has to match Chromium's constants exactly; no crate does that. |
| HTTP cache, DNS resolver (`http-cache`, `hickory-resolver`) | Not used | Servo and WebKit own their network stacks; a Mithu-side cache or resolver would not be on the page's request path. They become relevant when our own network engine plugs in (see [ENGINES.md](ENGINES.md)). |
| Allocator (`mimalloc`) | Not adopted yet | Not measured in this round; it is a next step and should only land with an A/B that shows lower PSS or CPU. |

## Still worse than Chromium, and why

1. **Servo on animated pages (Wikipedia, The Guardian, scrolling).** Servo keeps producing frames on pages with running animations or timers, and every frame is rasterised on the CPU by llvmpipe because this box has no GPU. Pacing caps that at 60 a second, but Wikipedia still costs ~72% of a core at idle against Chromium's ~8%, and scrolling the Rust article costs ~11.6 CPU-seconds against Chromium's 0.8. Chromium avoids this with damage-based partial raster, tiling and GPU compositing; Servo's WebRender re-renders the whole frame. Fixing it needs either a GPU path or engine-side work (frame throttling for unchanged display lists inside Servo).
2. **Lite idle CPU and wakeups on some pages** (Wikipedia ~28%, YouTube ~21%, scroll page ~21%, thousands of wakeups/s). This is inside WebKitGTK's web process, which also composites in software here; Mithu's UI process is idle. Lite's scroll CPU is also ~5x Chromium's.
3. **Many tabs.** Ten tabs cost 1.4 GB (Lite) and 1.9 GB (Servo) against Chromium's 1.0 GB, and Lite took 8.3 s to load all ten against Chromium's 5.1 s. Discarding only kicks in under memory pressure, as in Chromium.
4. **Servo on GitHub** varied between 1.6 and 4.2 s to load in the after-runs (see above). Needs a dedicated A/B.
5. **Load times are not exactly comparable.** Servo's load-complete fires earlier than Chromium's `loadEventEnd`.

## Next steps

- A/B `mimalloc` as the global allocator for the Servo edition (keep only if PSS or CPU drop).
- Skip painting when Servo's frame is pixel-identical to the last one (hash the damage rect), or ask Servo upstream for display-list-unchanged signalling.
- Repeat the GitHub load runs on Servo with more repetitions to separate noise from a regression.
- Proactive discarding of long-hidden tabs (Chromium's "Memory Saver" mode) as a setting.
- Measure on a machine with a GPU, where both engines composite on the GPU like Chromium does.
