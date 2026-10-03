#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
checkout=${1:-/home/nws/w/esp32/esp-rs/esp-hal}
patch="$repo_root/patches/esp-rtos-radio-affinity-s31.patch"
if git -C "$checkout" apply --reverse --check "$patch" 2>/dev/null; then
    printf '%s\n' 'S31 radio thread affinity workaround is already present.'
    exit 0
fi
git -C "$checkout" apply --check "$patch"
git -C "$checkout" apply "$patch"
printf '%s\n' 'Applied S31 radio thread affinity workaround; commit it in the dependency checkout.'
