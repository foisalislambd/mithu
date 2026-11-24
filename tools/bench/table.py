#!/usr/bin/env python3
"""Print Markdown tables of the medians in one or more bench.py JSON reports."""
import json
import sys

COLUMNS = [
    ("load_ms", "Load (ms)", 0),
    ("load_cpu_s", "Load CPU (s)", 2),
    ("peak_pss_mb", "Peak PSS (MB)", 0),
    ("settled_pss_mb", "Settled PSS (MB)", 0),
    ("settled_rss_mb", "Settled RSS (MB)", 0),
    ("idle_cpu_pct", "Idle CPU (%)", 1),
    ("idle_wakeups_per_s", "Wakeups/s", 0),
    ("scroll_cpu_s", "Scroll CPU (s)", 2),
    ("processes", "Procs", 0),
    ("timeouts", "Timeouts", 0),
]


def fmt(value, digits):
    if value is None:
        return "–"
    return f"{value:.{digits}f}" if digits else f"{round(value):d}"


def main():
    for path in sys.argv[1:]:
        report = json.load(open(path))
        print(f"<!-- {path}: started {report['started']}, {report['runs']} runs, {report['cpus']} CPUs -->")
        for scenario, cells in report["results"].items():
            rows = [(name, cell["median"]) for name, cell in cells.items() if "median" in cell]
            if not rows:
                continue
            used = [c for c in COLUMNS if any(m.get(c[0]) not in (None, 0) or c[0] == "load_ms" for _, m in rows)]
            print(f"\n**{scenario}**\n")
            print("| Browser | " + " | ".join(c[1] for c in used) + " |")
            print("|---|" + "---:|" * len(used))
            for name, median in rows:
                print(f"| {name} | " + " | ".join(fmt(median.get(c[0]), c[2]) for c in used) + " |")


if __name__ == "__main__":
    main()
