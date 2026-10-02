#!/usr/bin/env bash
set -euo pipefail
# Reproduce the exact upstream Zigbee stack + manifest-only S31 adapter port.
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
checkout=/home/nws/w/esp32/zigbee-rs-s31
revision=3c2d51c5ee18893e0e5bc4201e70a6358961708f
if test -e "$checkout"; then
    printf '%s\n' "Checkout exists: $checkout; inspect before changing it."
    exit 1
fi
git clone https://github.com/zigbee-rs/zigbee-rs.git "$checkout"
git -C "$checkout" checkout --detach "$revision"
git -C "$checkout" apply "$repo_root/patches/zigbee-rs-s31.patch"
printf '%s\n' "S31 adapter port applied to $checkout"
