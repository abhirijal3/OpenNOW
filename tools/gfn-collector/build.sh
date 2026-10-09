#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cargo build --release --manifest-path "$repo/native/opennow-core/Cargo.toml"
cargo build --release -p opennow-gfn --manifest-path "$repo/native/opennow-streamer/Cargo.toml"
mkdir -p "$here/build"
libs=(-lpthread -ldl -lm)
if [[ "$(uname)" == Darwin ]]; then
    libs=(-framework Security -framework CoreFoundation -framework SystemConfiguration)
fi
cc -std=c11 -O2 -Wall -Wextra -Werror -D_GNU_SOURCE \
    -I"$repo/native/opennow-streamer/crates/opennow-gfn/include" \
    "$here/collector.c" "$repo/native/opennow-streamer/target/release/libopennow_gfn.a" \
    "${libs[@]}" -o "$here/build/collector"
echo "built $here/build/collector"
