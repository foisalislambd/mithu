#!/bin/sh
# Resume a WPT run until mithu-measure exits 0.
# Exit 2 is a planned restart after 120 non-skip pages.
# Any other non-zero exit, or a URL left in the .current file, is one harness failure.
# A side loop kills the runner if RSS exceeds 2.5 GiB or one URL sits for 600s.
set -u
ROOT=${1:-/tmp/wpt}
LOG=${2:-/tmp/wpt-log.tsv}
BIN=${3:-target/release/mithu-measure}

(
  last=""
  same=0
  while true; do
    sleep 8
    cur=""
    if [ -f "$LOG.current" ]; then
      cur=$(cat "$LOG.current")
    fi
    if [ "$cur" = "$last" ]; then
      same=$((same + 8))
    else
      same=0
      last=$cur
    fi
    pid=$(pgrep -x mithu-measure | head -1)
    if [ -n "$pid" ] && [ -r "/proc/$pid/status" ]; then
      rss=$(awk '/VmRSS/{print $2}' "/proc/$pid/status")
      if [ "${rss:-0}" -gt 2500000 ] || [ "$same" -ge 180 ]; then
        echo "KILL pid=$pid rss=$rss stall=$same $cur" >> /tmp/wpt-score.err
        kill -9 "$pid" || true
        same=0
      fi
    fi
  done
) &
watch=$!
trap 'kill "$watch" 2>/dev/null || true' EXIT

set +e
attempt=1
while [ "$attempt" -le 400 ]; do
  env -u MITHU_JS_DEBUG -u MITHU_LOGS -u MITHU_TIMING \
    "$BIN" wpt "$ROOT" "$LOG" > /tmp/wpt-score.txt 2>> /tmp/wpt-score.err
  code=$?
  echo "ATTEMPT $attempt exit $code lines $(wc -l < "$LOG")" >> /tmp/wpt-score.err
  if [ "$code" = 0 ]; then
    echo SUPERVISOR_DONE >> /tmp/wpt-score.err
    exit 0
  fi
  last=$(tail -n 1 "$LOG" | cut -f1)
  cur=""
  if [ -f "$LOG.current" ]; then
    cur=$(cat "$LOG.current")
  fi
  if [ -n "$cur" ] && [ "$last" != "$cur" ]; then
    printf '%s\tharness\n' "$cur" >> "$LOG"
    echo "MARKED_CRASH $cur" >> /tmp/wpt-score.err
  fi
  attempt=$((attempt + 1))
done
echo SUPERVISOR_EXHAUSTED >> /tmp/wpt-score.err
exit 1
