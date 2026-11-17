#!/bin/sh
# Generate the phone new-tab bundle from mithu-mobile. The HTML is not committed.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
dest=${1:?usage: tools/install-mobile-assets.sh DIRECTORY}
cd "$root"
sh tools/ensure-servo-manifest.sh
cargo run -q -p mithu-mobile --bin mithu-mobile-assets -- "$dest"
