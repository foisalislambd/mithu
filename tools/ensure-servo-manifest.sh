#!/bin/sh
# Cargo resolves every workspace member, including mithu-shell's path to
# Servo. The phone jobs do not compile Servo. If the submodule is absent,
# leave a manifest that only advertises the features the shell asks for.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
manifest="$root/third_party/servo/components/servo/Cargo.toml"
if [ -f "$manifest" ]; then
  exit 0
fi
mkdir -p "$(dirname "$manifest")/src"
cat > "$manifest" << 'EOF'
[package]
name = "servo"
version = "0.0.0"
edition = "2021"
publish = false

[features]
default = []
bundled = []
clipboard = []
js_jit = []
multiprocess = []
no-wgl = []
EOF
printf '%s\n' '// Resolver stub. Not compiled into the phone apps.' > "$(dirname "$manifest")/src/lib.rs"
allocator="$root/third_party/servo/components/allocator/Cargo.toml"
if [ ! -f "$allocator" ]; then
  mkdir -p "$(dirname "$allocator")"
  cat > "$allocator" << 'EOF'
[package]
name = "servo-allocator"
version = "0.8.0"
edition = "2021"
publish = false

[features]
use-jemalloc = []
EOF
  printf '%s\n' '// Resolver stub. Not compiled into the phone apps.' > "$(dirname "$allocator")/lib.rs"
fi
