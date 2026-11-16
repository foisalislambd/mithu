#!/bin/sh
# Build a release binary. `lite` does not configure or compile Servo.
# `full` needs the Servo submodule (depth 1). `both` builds the two packages
# as separate artifacts; they are not linked into one binary.
set -eu
cd "$(dirname "$0")/.."
kind="${1:-lite}"

build_lite() {
  cargo build -p mithu-lite --release
  echo "artifact target/release/mithu-lite"
}

build_full() {
  git submodule update --init --depth 1 third_party/servo
  cargo build -p mithu --release
  echo "artifact target/release/mithu"
}

case "$kind" in
  lite) build_lite ;;
  full) build_full ;;
  both)
    build_lite
    build_full
    ;;
  *)
    echo "usage: tools/package.sh [lite|full|both]" >&2
    exit 2
    ;;
esac
