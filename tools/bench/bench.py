#!/usr/bin/env python3
"""Mithu vs Chromium resource benchmark (Linux, stdlib only).

Every browser is started the same way:
  * on its own Xvfb display (1280x800x24, no window manager, no GPU),
  * with a fresh profile and a fresh $HOME (cold HTTP cache),
  * with the same window size (1280x800).

The harness owns the clock. "load" is the wall-clock time from spawning the
browser to the page's load event:
  * Chromium: `performance.timing.loadEventEnd` read over the DevTools
    protocol (every page target, so N tabs wait for all N),
  * Mithu: the `bench loaded <unix ms> <url>` line both editions print when
    MITHU_BENCH=1 and a tab first reaches its load-complete state
    (Servo `LoadStatus::Complete`, WebKitGTK `PageLoadEvent::Finished`).

Memory and CPU are summed over the whole browser tree: every process that
descends from the launched PID, plus any process that carries this run's
MITHU_BENCH_TAG in its environment (catches processes a zygote or bwrap
reparents). RSS comes from /proc/PID/status, PSS from /proc/PID/smaps_rollup
(PSS splits shared pages between processes, so it does not double-count the
shared libraries every Chromium child maps). CPU is utime+stime from
/proc/PID/stat, keeping the last value seen for processes that exit.
Context switches (voluntary + involuntary) during the idle window stand in
for Chromium's "idle wakeups" metric.
"""

import argparse
import base64
import json
import os
import shutil
import signal
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import uuid

TICK = os.sysconf("SC_CLK_TCK")
PAGE_KB = os.sysconf("SC_PAGE_SIZE") // 1024
WIDTH, HEIGHT = 1280, 800

SITES = [
    "https://example.com/",
    "https://en.wikipedia.org/wiki/Main_Page",
    "https://github.com/foisalislambd/mithu",
    "https://news.ycombinator.com/",
    "https://www.youtube.com/",
    "https://www.theguardian.com/international",
]

TEN_TABS = [
    "https://example.com/",
    "https://en.wikipedia.org/wiki/Main_Page",
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://en.wikipedia.org/wiki/Bangladesh",
    "https://news.ycombinator.com/",
    "https://github.com/foisalislambd/mithu",
    "https://www.rust-lang.org/",
    "https://developer.mozilla.org/en-US/",
    "https://www.theguardian.com/international",
    "https://docs.rs/",
]

SCROLL_URL = "https://en.wikipedia.org/wiki/Rust_(programming_language)"


# ---------------------------------------------------------------- /proc reads

def read(path):
    try:
        with open(path, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def ppid_map():
    parents = {}
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        stat = read(f"/proc/{name}/stat")
        if not stat:
            continue
        tail = stat[stat.rfind(b")") + 2:].split()
        parents[int(name)] = int(tail[1])
    return parents


def tagged(tag):
    needle = f"MITHU_BENCH_TAG={tag}".encode()
    found = set()
    for name in os.listdir("/proc"):
        if name.isdigit():
            env = read(f"/proc/{name}/environ")
            if env and needle in env:
                found.add(int(name))
    return found


def tree(root, tag):
    parents = ppid_map()
    members = {root} | tagged(tag)
    grew = True
    while grew:
        grew = False
        for pid, parent in parents.items():
            if parent in members and pid not in members:
                members.add(pid)
                grew = True
    return {pid for pid in members if pid in parents}


def proc_sample(pid):
    stat = read(f"/proc/{pid}/stat")
    status = read(f"/proc/{pid}/status")
    if not stat or not status:
        return None
    tail = stat[stat.rfind(b")") + 2:].split()
    ticks = int(tail[11]) + int(tail[12])
    rss = 0
    for line in status.split(b"\n"):
        if line.startswith(b"VmRSS:"):
            rss = int(line.split()[1])
            break
    # /proc/PID/status only counts the main thread's switches; sum every task.
    switches = 0
    try:
        tasks = os.listdir(f"/proc/{pid}/task")
    except OSError:
        tasks = []
    for task in tasks:
        task_status = read(f"/proc/{pid}/task/{task}/status") or b""
        for line in task_status.split(b"\n"):
            if line.startswith(b"voluntary_ctxt_switches:") or line.startswith(b"nonvoluntary_ctxt_switches:"):
                switches += int(line.split()[1])
    pss = None
    rollup = read(f"/proc/{pid}/smaps_rollup")
    if rollup:
        for line in rollup.split(b"\n"):
            if line.startswith(b"Pss:"):
                pss = int(line.split()[1])
                break
    comm = (read(f"/proc/{pid}/comm") or b"?").strip().decode(errors="replace")
    return {"ticks": ticks, "rss": rss, "pss": pss, "switches": switches, "comm": comm}


class Sampler:
    """Samples the browser tree every `period` seconds on a thread."""

    def __init__(self, root, tag, period=0.25):
        self.root, self.tag, self.period = root, tag, period
        self.samples = []  # (t, rss_kb, pss_kb, pss_complete, procs)
        self.cpu = {}  # pid -> last ticks
        self.switches = {}  # pid -> last switches
        self.names = {}
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def start(self):
        self.thread.start()
        return self

    def run(self):
        while not self.stop.is_set():
            self.once()
            self.stop.wait(self.period)

    def once(self):
        now = time.monotonic()
        rss = pss = 0
        complete = True
        pids = tree(self.root, self.tag)
        for pid in pids:
            sample = proc_sample(pid)
            if not sample:
                continue
            self.cpu[pid] = sample["ticks"]
            self.switches[pid] = sample["switches"]
            self.names[pid] = sample["comm"]
            rss += sample["rss"]
            if sample["pss"] is None:
                complete = False
                pss += sample["rss"]
            else:
                pss += sample["pss"]
        self.samples.append((now, rss, pss, complete, len(pids)))

    def cpu_seconds(self):
        return sum(self.cpu.values()) / TICK

    def total_switches(self):
        return sum(self.switches.values())

    def finish(self):
        self.stop.set()
        self.thread.join()


# ------------------------------------------------------------ tiny websocket

class DevTools:
    """Just enough RFC 6455 to send CDP commands and read the replies."""

    def __init__(self, ws_url):
        rest = ws_url.split("://", 1)[1]
        hostport, path = rest.split("/", 1)
        host, port = hostport.split(":")
        self.sock = socket.create_connection((host, int(port)), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            (f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\n"
             f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise OSError("handshake closed")
            response += chunk
        self.buffer = response.split(b"\r\n\r\n", 1)[1]
        self.next_id = 0

    def send(self, payload):
        data = json.dumps(payload).encode()
        mask = os.urandom(4)
        header = bytes([0x81])
        if len(data) < 126:
            header += bytes([0x80 | len(data)])
        elif len(data) < 65536:
            header += bytes([0x80 | 126]) + struct.pack(">H", len(data))
        else:
            header += bytes([0x80 | 127]) + struct.pack(">Q", len(data))
        masked = bytes(byte ^ mask[i % 4] for i, byte in enumerate(data))
        self.sock.sendall(header + mask + masked)

    def recv_exact(self, count):
        while len(self.buffer) < count:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise OSError("closed")
            self.buffer += chunk
        out, self.buffer = self.buffer[:count], self.buffer[count:]
        return out

    def recv(self):
        message = b""
        while True:
            first, second = self.recv_exact(2)
            length = second & 0x7F
            if length == 126:
                length = struct.unpack(">H", self.recv_exact(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", self.recv_exact(8))[0]
            message += self.recv_exact(length)
            if first & 0x80:
                return json.loads(message)

    def call(self, method, params=None):
        self.next_id += 1
        self.send({"id": self.next_id, "method": method, "params": params or {}})
        while True:
            reply = self.recv()
            if reply.get("id") == self.next_id:
                return reply

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def chrome_load_times(port, want):
    """Epoch ms of loadEventEnd for each http(s) page target, or None."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=2) as response:
            targets = json.load(response)
    except OSError:
        return None
    pages = [t for t in targets if t.get("type") == "page" and t.get("url", "").startswith("http")]
    if len(pages) < want:
        return None
    ends = []
    for target in pages:
        try:
            tools = DevTools(target["webSocketDebuggerUrl"])
            reply = tools.call("Runtime.evaluate", {
                "expression": "performance.timing.loadEventEnd", "returnByValue": True})
            tools.close()
            value = reply.get("result", {}).get("result", {}).get("value", 0)
        except (OSError, ValueError, KeyError):
            return None
        if not value:
            return None
        ends.append(value)
    return ends


# ------------------------------------------------------------------ browsers

def chrome_command(binary, profile, port, urls):
    return [binary, f"--user-data-dir={profile}", f"--remote-debugging-port={port}",
            "--no-first-run", "--no-default-browser-check", "--password-store=basic",
            "--disable-features=Translate", f"--window-size={WIDTH},{HEIGHT}",
            "--window-position=0,0"] + urls


def mithu_command(binary, urls):
    return [binary, f"--window-size={WIDTH},{HEIGHT}"] + urls


class Run:
    def __init__(self, browser, binary, urls, display):
        self.browser, self.binary, self.urls = browser, binary, urls
        self.home = tempfile.mkdtemp(prefix="mithu-bench-home-")
        self.tag = uuid.uuid4().hex
        self.port = 9300 + (os.getpid() % 500)
        env = dict(os.environ)
        env.update({"HOME": self.home, "DISPLAY": display, "MITHU_BENCH": "1",
                    "MITHU_BENCH_TAG": self.tag, "XDG_CONFIG_HOME": f"{self.home}/.config",
                    "XDG_DATA_HOME": f"{self.home}/.local/share", "XDG_CACHE_HOME": f"{self.home}/.cache",
                    "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "1"})
        env.pop("WAYLAND_DISPLAY", None)
        if browser == "chromium":
            command = chrome_command(binary, f"{self.home}/profile", self.port, urls)
        else:
            command = mithu_command(binary, urls)
        self.loaded = []
        self.spawn_epoch_ms = time.time() * 1000
        self.spawn_mono = time.monotonic()
        self.proc = subprocess.Popen(command, env=env, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, start_new_session=True)
        self.sampler = Sampler(self.proc.pid, self.tag).start()
        if browser != "chromium":
            threading.Thread(target=self.read_stdout, daemon=True).start()

    def read_stdout(self):
        for raw in self.proc.stdout:
            parts = raw.decode(errors="replace").split()
            if len(parts) >= 3 and parts[0] == "bench" and parts[1] == "loaded":
                self.loaded.append(float(parts[2]))

    def wait_loaded(self, timeout):
        """Returns load ms (spawn -> last of N load events) or None on timeout."""
        want = len(self.urls)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                return None
            if self.browser == "chromium":
                ends = chrome_load_times(self.port, want)
                if ends:
                    return max(ends) - self.spawn_epoch_ms
            elif len(self.loaded) >= want:
                return max(self.loaded[:want]) - self.spawn_epoch_ms
            time.sleep(0.2)
        return None

    def stop(self):
        self.sampler.finish()
        pids = tree(self.proc.pid, self.tag)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for pid in pids:
                try:
                    os.kill(pid, sig)
                except OSError:
                    pass
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
        time.sleep(0.5)
        for pid in tree(self.proc.pid, self.tag):
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
        shutil.rmtree(self.home, ignore_errors=True)


def measure(browser, binary, urls, display, idle=10.0, timeout=60.0, settle=0.0, scroll=False):
    run = Run(browser, binary, urls, display)
    result = {"browser": browser, "urls": urls}
    try:
        if urls:
            load_ms = run.wait_loaded(timeout)
        else:
            time.sleep(settle)
            load_ms = None
        result["load_ms"] = None if load_ms is None else round(load_ms)
        result["timeout"] = bool(urls) and load_ms is None
        load_end = time.monotonic()
        cpu_load = run.sampler.cpu_seconds()
        result["load_cpu_s"] = round(cpu_load, 3)
        wall = load_end - run.spawn_mono
        result["load_cpu_pct"] = round(100.0 * cpu_load / wall, 1) if wall > 0 else None
        if scroll:
            time.sleep(2.0)
            cpu0, t0 = run.sampler.cpu_seconds(), time.monotonic()
            subprocess.run(["xdotool", "mousemove", str(WIDTH // 2), str(HEIGHT // 2 + 100)],
                           env={**os.environ, "DISPLAY": display})
            subprocess.run(["xdotool", "click", "--repeat", "40", "--delay", "50", "5"],
                           env={**os.environ, "DISPLAY": display})
            subprocess.run(["xdotool", "click", "--repeat", "40", "--delay", "50", "4"],
                           env={**os.environ, "DISPLAY": display})
            time.sleep(0.5)
            run.sampler.once()
            elapsed = time.monotonic() - t0
            result["scroll_cpu_s"] = round(run.sampler.cpu_seconds() - cpu0, 3)
            result["scroll_wall_s"] = round(elapsed, 2)
        # Two seconds for post-load work (late scripts, first paint of
        # below-the-fold images) before the idle window opens.
        time.sleep(2.0)
        run.sampler.once()
        cpu0, sw0, t0 = run.sampler.cpu_seconds(), run.sampler.total_switches(), time.monotonic()
        time.sleep(idle)
        run.sampler.once()
        elapsed = time.monotonic() - t0
        result["idle_cpu_pct"] = round(100.0 * (run.sampler.cpu_seconds() - cpu0) / elapsed, 2)
        result["idle_wakeups_per_s"] = round((run.sampler.total_switches() - sw0) / elapsed, 1)
        samples = run.sampler.samples
        tail = [s for s in samples if s[0] >= time.monotonic() - 3.5]
        result["peak_rss_mb"] = round(max(s[1] for s in samples) / 1024, 1)
        result["peak_pss_mb"] = round(max(s[2] for s in samples) / 1024, 1)
        result["settled_rss_mb"] = round(statistics.median(s[1] for s in tail) / 1024, 1)
        result["settled_pss_mb"] = round(statistics.median(s[2] for s in tail) / 1024, 1)
        result["pss_complete"] = all(s[3] for s in tail)
        result["processes"] = tail[-1][4] if tail else None
        result["process_names"] = sorted(set(run.sampler.names.values()))
    finally:
        run.stop()
    return result


def start_display(number):
    display = f":{number}"
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.0)
    return display, xvfb


def median_of(runs, key):
    values = [r[key] for r in runs if r.get(key) is not None]
    return round(statistics.median(values), 2) if values else None


KEYS = ["load_ms", "load_cpu_s", "load_cpu_pct", "peak_rss_mb", "peak_pss_mb", "settled_rss_mb",
        "settled_pss_mb", "idle_cpu_pct", "idle_wakeups_per_s", "scroll_cpu_s", "processes"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", action="append", required=True,
                        help="name=path, e.g. chromium=/usr/bin/google-chrome mithu=target/release/mithu")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--scenario", action="append",
                        help="blank, tabs10, scroll, or a site URL; default: everything")
    parser.add_argument("--idle", type=float, default=10.0)
    parser.add_argument("--display", type=int, default=77)
    parser.add_argument("--out", required=True, help="JSON file for raw runs + medians")
    args = parser.parse_args()

    browsers = [entry.split("=", 1) for entry in args.browser]
    scenarios = args.scenario or (["blank"] + SITES + ["tabs10", "scroll"])
    display, xvfb = start_display(args.display)
    report = {"started": time.strftime("%Y-%m-%d %H:%M:%S %z"), "host": os.uname().nodename,
              "cpus": os.cpu_count(), "window": f"{WIDTH}x{HEIGHT}", "runs": args.runs,
              "browsers": {name: path for name, path in browsers}, "results": {}}
    try:
        for scenario in scenarios:
            for name, path in browsers:
                report["results"].setdefault(scenario, {})[name] = {"raw": []}
            # Interleave browsers inside each repetition so network drift hits all of them.
            for repetition in range(args.runs):
                for name, path in browsers:
                    kind = "chromium" if "chrom" in name or "chrome" in path else "mithu"
                    if scenario == "blank":
                        result = measure(kind, path, [], display, idle=args.idle, settle=5.0)
                    elif scenario == "tabs10":
                        result = measure(kind, path, TEN_TABS, display, idle=args.idle, timeout=90)
                    elif scenario == "scroll":
                        result = measure(kind, path, [SCROLL_URL], display, idle=args.idle, scroll=True)
                    else:
                        result = measure(kind, path, [scenario], display, idle=args.idle)
                    report["results"][scenario][name]["raw"].append(result)
                    print(json.dumps({"scenario": scenario, "browser": name, "rep": repetition,
                                      **{k: result.get(k) for k in KEYS + ["timeout"]}}), flush=True)
                    time.sleep(1.0)
            for name, _ in browsers:
                cell = report["results"][scenario][name]
                cell["median"] = {key: median_of(cell["raw"], key) for key in KEYS}
                cell["median"]["timeouts"] = sum(1 for r in cell["raw"] if r.get("timeout"))
            with open(args.out, "w") as handle:
                json.dump(report, handle, indent=1)
    finally:
        xvfb.terminate()
    with open(args.out, "w") as handle:
        json.dump(report, handle, indent=1)


if __name__ == "__main__":
    main()
