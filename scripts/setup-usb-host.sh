#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
embassy_checkout=${1:-/home/nws/w/esp32/embassy-usb-host}
hal_checkout=${2:-/home/nws/w/esp32/esp-rs/esp-hal}
revision=ae258ddd1b2a45715aef5ac12a70434b94e96139
if ! test -e "$embassy_checkout"; then
    git clone --no-checkout https://github.com/embassy-rs/embassy.git "$embassy_checkout"
    git -C "$embassy_checkout" checkout --detach "$revision"
fi
apply_once() {
    local checkout=$1 patch_file=$2
    if git -C "$checkout" apply --reverse --check "$patch_file" 2>/dev/null; then
        printf '%s\n' "Patch already present: $patch_file"
        return
    fi
    git -C "$checkout" apply --check "$patch_file"
    git -C "$checkout" apply "$patch_file"
}
git -C "$embassy_checkout" merge-base --is-ancestor "$revision" HEAD
apply_once "$embassy_checkout" "$repo_root/patches/embassy-usb-host-s31.patch"
apply_once "$hal_checkout" "$repo_root/patches/esp-hal-usb-host-s31.patch"
printf '%s\n' 'USB patches applied. Commit them in the dependency checkouts.'
printf '%s\n' 'Cargo.toml paths must match the selected checkout locations.'
