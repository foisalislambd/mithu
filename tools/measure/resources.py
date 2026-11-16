#!/usr/bin/env python3
"""Resource samples for the Mithu release binary, Chrome, and Firefox.

One fresh process per case. Samples come from /proc every 100ms.

VmRSS is summed across the process and its descendants. Shared pages are
counted once per process. CPU time is utime+stime from /proc/pid/stat,
converted with SC_CLK_TCK, also summed across the tree.

Mithu load time is the wall time until the release binary prints `loaded`.
That line is after navigation and the same quiet-pump the screenshot harness
uses, at 1280x800. The process then stays up for 12 seconds. Steady RSS is
the sample closest to 10 seconds after `loaded`. CPU-to-load is the tree's
user+sys time at the first sample at or after `loaded`.

Chrome and Firefox, when present, are started headless at 1280x800. Load time
is the wall time until the remote-debugging target reports a URL that is no
longer about:blank (or, for about:blank itself, until the first target exists).
The browser is killed 12 seconds after that. Steady RSS is 10 seconds after
load. A five-tab sample opens five about:blank targets.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
import urllib.request
from pathlib import Path

TICKS = os.sysconf("SC_CLK_TCK")
ROOT = Path("/tmp/mithu-rss")
MITHU = Path("/agent/repos/mithu/target/release/mithu")
CHROME = shutil.which("google-chrome") or "/usr/local/bin/google-chrome"
def firefox_binary() -> str | None:
    for candidate in (shutil.which("firefox"), shutil.which("firefox-esr"), "/tmp/firefox/firefox"):
        if candidate and Path(candidate).exists():
            return candidate
    return None


FIREFOX = firefox_binary()
SITES = [
    ("example.com", "https://example.com/"),
    ("en.wikipedia.org", "https://en.wikipedia.org/wiki/Main_Page"),
    ("react.dev", "https://react.dev/"),
    ("github.com", "https://github.com/"),
    ("youtube.com", "https://www.youtube.com/"),
    ("bbc.com", "https://www.bbc.com/"),
]


def tree_pids(root: int) -> list[int]:
    children: dict[int, list[int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        rest = stat.rsplit(")", 1)[-1].split()
        if len(rest) < 2 or not rest[1].isdigit():
            continue
        children.setdefault(int(rest[1]), []).append(int(entry.name))
    out = [root]
    stack = [root]
    while stack:
        pid = stack.pop()
        for child in children.get(pid, []):
            out.append(child)
            stack.append(child)
    return out


def read_tree(pid: int) -> tuple[int, int, float] | None:
    pids = tree_pids(pid)
    rss = 0
    threads = 0
    cpu = 0.0
    alive = False
    for item in pids:
        try:
            status = Path(f"/proc/{item}/status").read_text()
            stat = Path(f"/proc/{item}/stat").read_text()
        except OSError:
            continue
        alive = True
        for line in status.splitlines():
            if line.startswith("VmRSS:"):
                rss += int(line.split()[1])
            elif line.startswith("Threads:"):
                threads += int(line.split()[1])
        rest = stat.rsplit(")", 1)[-1].split()
        if len(rest) > 12:
            cpu += (int(rest[11]) + int(rest[12])) / TICKS
    if not alive:
        return None
    return rss, threads, cpu


def watch(pid: int, samples: list[dict], stop: list[bool]) -> None:
    while not stop[0]:
        row = read_tree(pid)
        if row is None:
            break
        rss, threads, cpu = row
        samples.append(
            {
                "t": time.time(),
                "rss_kib": rss,
                "threads": threads,
                "cpu_s": cpu,
            }
        )
        time.sleep(0.1)


def summarize(samples: list[dict], start: float, loaded: float | None) -> dict:
    if not samples:
        return {"error": "no samples"}
    peak = max(samples, key=lambda row: row["rss_kib"])
    steady = None
    cpu_load = None
    if loaded is not None:
        target = loaded + 10
        steady = min(samples, key=lambda row: abs(row["t"] - target))
        after = [row for row in samples if row["t"] >= loaded]
        if after:
            cpu_load = after[0]["cpu_s"]
    return {
        "peak_rss_kib": peak["rss_kib"],
        "peak_rss_mib": round(peak["rss_kib"] / 1024, 1),
        "steady_rss_kib": None if steady is None else steady["rss_kib"],
        "steady_rss_mib": None if steady is None else round(steady["rss_kib"] / 1024, 1),
        "steady_threads": None if steady is None else steady["threads"],
        "cpu_to_load_s": None if cpu_load is None else round(cpu_load, 3),
        "wall_load_s": None if loaded is None else round(loaded - start, 3),
        "samples": len(samples),
    }


def run_mithu(url: str, tabs: int) -> dict:
    samples: list[dict] = []
    stop = [False]
    start = time.time()
    proc = subprocess.Popen(
        [str(MITHU), "--hold", "12", url, str(tabs)],
        stderr=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        text=True,
    )
    import threading

    thread = threading.Thread(target=watch, args=(proc.pid, samples, stop))
    thread.start()
    loaded = None
    assert proc.stderr is not None
    for line in proc.stderr:
        if line.startswith("loaded "):
            loaded = time.time()
            break
    proc.wait(timeout=60)
    stop[0] = True
    thread.join(timeout=2)
    result = summarize(samples, start, loaded)
    result["tabs"] = tabs
    result["url"] = url
    return result


def browser_targets(port: int) -> list[dict]:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/json", timeout=0.5) as response:
            return json.load(response)
    except Exception:
        return []


def run_cdp(binary: str, args: list[str], urls: list[str], port: int) -> dict:
    """One headless process.

    Load time for a single URL is a separate `--dump-dom` process: it exits
    when the DOM is available. The held process is the RSS sample. Its steady
    point is `dump_wall + 10s` after it starts, using the dump-dom duration as
    the load time. about:blank has no dump; load is the first debugging target.
    """
    import threading
    import urllib.parse

    dump_wall = None
    dump_cpu = None
    dump_peak = None
    if urls != ["about:blank"] and not (len(urls) > 1 and urls[0] == "about:blank"):
        dump = measure_dump(binary, args, urls[0])
        dump_wall = dump["wall_load_s"]
        dump_cpu = dump["cpu_to_load_s"]
        dump_peak = dump["peak_rss_kib"]

    profile = ROOT / f"profile-{port}"
    if profile.exists():
        shutil.rmtree(profile, ignore_errors=True)
    profile.mkdir(parents=True)
    command = [
        binary,
        *args,
        f"--remote-debugging-port={port}",
        f"--user-data-dir={profile}",
        "--window-size=1280,800",
        urls[0],
    ]
    samples: list[dict] = []
    stop = [False]
    start = time.time()
    proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    thread = threading.Thread(target=watch, args=(proc.pid, samples, stop))
    thread.start()
    opened = 0
    if len(urls) > 1:
        deadline = time.time() + 8
        while time.time() < deadline and not browser_targets(port):
            time.sleep(0.1)
        for url in urls[1:]:
            quoted = urllib.parse.quote(url, safe=":/")
            try:
                request = urllib.request.Request(
                    f"http://127.0.0.1:{port}/json/new?{quoted}",
                    method="PUT",
                )
                urllib.request.urlopen(request, timeout=3).read()
                opened += 1
            except Exception:
                pass
    loaded = start if dump_wall is None else start + dump_wall
    time.sleep(max(0.0, (loaded + 12) - time.time()))
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    stop[0] = True
    thread.join(timeout=2)
    result = summarize(samples, start, loaded)
    if dump_wall is not None:
        result["wall_load_s"] = dump_wall
        result["cpu_to_load_s"] = dump_cpu
        result["dump_peak_rss_kib"] = dump_peak
        result["peak_rss_kib"] = max(result["peak_rss_kib"], dump_peak)
        result["peak_rss_mib"] = round(result["peak_rss_kib"] / 1024, 1)
    result["url"] = urls[0]
    result["tabs"] = 1 + opened if len(urls) > 1 else 1
    result["cdp_tabs_opened"] = opened
    return result


def measure_dump(binary: str, args: list[str], url: str) -> dict:
    """Chrome prints the DOM and then stays up. Load time is until `</html>`."""
    import select
    import threading

    profile = ROOT / "dump-profile"
    if profile.exists():
        shutil.rmtree(profile, ignore_errors=True)
    samples: list[dict] = []
    stop = [False]
    start = time.time()
    proc = subprocess.Popen(
        [
            binary,
            *args,
            f"--user-data-dir={profile}",
            "--window-size=1280,800",
            "--timeout=30000",
            "--dump-dom",
            url,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    thread = threading.Thread(target=watch, args=(proc.pid, samples, stop))
    thread.start()
    fd = proc.stdout.fileno() if proc.stdout is not None else -1
    buf = b""
    closed = False
    while time.time() - start < 45 and fd >= 0:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if not ready:
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
        if b"</html>" in buf.lower():
            closed = True
            break
    end = time.time()
    proc.kill()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    stop[0] = True
    thread.join(timeout=2)
    peak = max((row["rss_kib"] for row in samples), default=0)
    cpu = samples[-1]["cpu_s"] if samples else 0.0
    return {
        "wall_load_s": round(end - start, 3),
        "cpu_to_load_s": round(cpu, 3),
        "peak_rss_kib": peak,
        "dump_bytes": len(buf),
        "dump_closed": closed,
    }


def measure_screenshot(binary: str, url: str) -> dict:
    """Firefox --screenshot exits after the page is painted. Cold start included."""
    import threading

    profile = ROOT / "ff-shot-profile"
    if profile.exists():
        shutil.rmtree(profile, ignore_errors=True)
    profile.mkdir(parents=True, exist_ok=True)
    shot = ROOT / "ff-shot.png"
    samples: list[dict] = []
    stop = [False]
    start = time.time()
    proc = subprocess.Popen(
        [
            binary,
            "--headless",
            "--no-remote",
            f"--profile={profile}",
            "--window-size",
            "1280,800",
            "--screenshot",
            str(shot),
            url,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    thread = threading.Thread(target=watch, args=(proc.pid, samples, stop))
    thread.start()
    try:
        proc.wait(timeout=45)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)
    end = time.time()
    stop[0] = True
    thread.join(timeout=2)
    peak = max((row["rss_kib"] for row in samples), default=0)
    cpu = samples[-1]["cpu_s"] if samples else 0.0
    return {
        "wall_load_s": round(end - start, 3),
        "cpu_to_load_s": round(cpu, 3),
        "peak_rss_kib": peak,
        "screenshot_bytes": shot.stat().st_size if shot.exists() else 0,
    }


def run_firefox(binary: str, urls: list[str], port: int) -> dict:
    import threading

    shot = None
    if urls != ["about:blank"] and not (len(urls) > 1 and urls[0] == "about:blank"):
        shot = measure_screenshot(binary, urls[0])
    profile = ROOT / f"ff-profile-{port}"
    if profile.exists():
        shutil.rmtree(profile, ignore_errors=True)
    profile.mkdir(parents=True)
    command = [
        binary,
        "--headless",
        "--no-remote",
        f"--profile={profile}",
        "--window-size",
        "1280,800",
        urls[0],
    ]
    for _ in urls[1:]:
        command.extend(["--new-tab", "about:blank"])
    samples: list[dict] = []
    stop = [False]
    start = time.time()
    proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    thread = threading.Thread(target=watch, args=(proc.pid, samples, stop))
    thread.start()
    loaded = start if shot is None else start + shot["wall_load_s"]
    time.sleep(max(0.0, (loaded + 12) - time.time()))
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    stop[0] = True
    thread.join(timeout=2)
    # bidi errors were stuffed into samples as non-numeric rows; drop them.
    numeric = [row for row in samples if "rss_kib" in row]
    result = summarize(numeric, start, loaded)
    if shot is not None:
        result["wall_load_s"] = shot["wall_load_s"]
        result["cpu_to_load_s"] = shot["cpu_to_load_s"]
        result["shot_peak_rss_kib"] = shot["peak_rss_kib"]
        result["screenshot_bytes"] = shot["screenshot_bytes"]
        result["peak_rss_kib"] = max(result.get("peak_rss_kib") or 0, shot["peak_rss_kib"])
        result["peak_rss_mib"] = round(result["peak_rss_kib"] / 1024, 1)
    result["url"] = urls[0]
    result["tabs"] = len(urls)
    result["load_event"] = "screenshot" if shot else "process-start"
    return result


def main() -> None:
    import urllib.parse

    ROOT.mkdir(exist_ok=True)
    report: dict = {
        "ticks": TICKS,
        "mithu_binary_bytes": MITHU.stat().st_size,
        "chrome_binary": CHROME,
        "firefox_binary": FIREFOX,
        "chrome_version": None,
        "firefox_version": None,
        "chrome_install_bytes": None,
        "firefox_install_bytes": None,
        "mithu": {},
        "chrome": {},
        "firefox": {},
    }
    if Path(CHROME).exists():
        report["chrome_version"] = subprocess.check_output([CHROME, "--version"], text=True).strip()
        chrome_bin = Path("/opt/google/chrome/chrome")
        if not chrome_bin.exists():
            chrome_bin = Path(CHROME).resolve()
        report["chrome_install_bytes"] = du(chrome_bin.parent)
    if FIREFOX:
        report["firefox_version"] = subprocess.check_output([FIREFOX, "--version"], text=True).strip()
        report["firefox_install_bytes"] = du(Path(FIREFOX).resolve().parent)
    cases = [("about:blank", "about:blank", 1), ("about:blank-5", "about:blank", 5)]
    cases += [(name, url, 1) for name, url in SITES]
    previous = Path("/tmp/mithu-rss.json")
    if os.environ.get("SKIP_MITHU") == "1" and previous.exists():
        report["mithu"] = json.loads(previous.read_text()).get("mithu", {})
        print("kept previous mithu samples", flush=True)
    else:
        for name, url, tabs in cases:
            print("mithu", name, flush=True)
            report["mithu"][name] = run_mithu(url, tabs)
            print(report["mithu"][name], flush=True)
    chrome_args = ["--headless=new", "--disable-gpu", "--no-sandbox", "--disable-dev-shm-usage"]
    port = 9333
    if Path(CHROME).exists():
        for name, url, tabs in cases:
            urls = ["about:blank"] * tabs if url == "about:blank" else [url]
            print("chrome", name, flush=True)
            report["chrome"][name] = run_cdp(CHROME, chrome_args, urls, port)
            port += 1
            print(report["chrome"][name], flush=True)
    if FIREFOX:
        for name, url, tabs in cases:
            urls = ["about:blank"] * tabs if url == "about:blank" else [url]
            print("firefox", name, flush=True)
            report["firefox"][name] = run_firefox(FIREFOX, urls, port)
            port += 1
            print(report["firefox"][name], flush=True)
    Path("/tmp/mithu-rss.json").write_text(json.dumps(report, indent=2))
    print("wrote /tmp/mithu-rss.json")


def du(path: Path) -> int:
    total = 0
    if path.is_file():
        return path.stat().st_size
    for item in path.rglob("*"):
        if item.is_file():
            try:
                total += item.stat().st_size
            except OSError:
                pass
    return total


if __name__ == "__main__":
    main()
