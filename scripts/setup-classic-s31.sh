#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
hal_checkout=${1:-/home/nws/w/esp32/esp-rs/esp-hal}
sys_checkout=${2:-/home/nws/w/esp32/esp-rs/esp-wifi-sys-s31-classic}

# Recreate only the published, checksum-pinned SDK source package. Never mix
# newer IDF BR/EDR archives with its older config and HCI packet ABI.
if ! test -e "$sys_checkout"; then
    python3 - "$sys_checkout" <<'PY'
import hashlib, io, pathlib, sys, tarfile, urllib.request
archive = urllib.request.urlopen('https://static.crates.io/crates/esp-wifi-sys-esp32s31/esp-wifi-sys-esp32s31-0.3.0.crate').read()
expected = 'ce987bf209ad4d95810c206acce9505d34056758ab5537ed8ced798945dc4131'
if hashlib.sha256(archive).hexdigest() != expected:
    raise SystemExit('S31 SDK package checksum mismatch')
destination = pathlib.Path(sys.argv[1])
with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as package:
    members = package.getmembers()
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if path.parts[0] != 'esp-wifi-sys-esp32s31-0.3.0' or '..' in path.parts or member.issym() or member.islnk():
            raise SystemExit('Unsafe SDK archive path')
    destination.mkdir(parents=True)
    for member in members:
        relative = pathlib.PurePosixPath(member.name).relative_to('esp-wifi-sys-esp32s31-0.3.0')
        target = destination.joinpath(*relative.parts)
        if member.isdir():
            target.mkdir(parents=True, exist_ok=True)
        elif member.isfile():
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(package.extractfile(member).read())
PY
    git -C "$sys_checkout" init -q
    git -C "$sys_checkout" add .
    git -C "$sys_checkout" commit -qm 'Snapshot checksum-pinned S31 SDK crate 0.3.0'
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
apply_once "$sys_checkout" "$repo_root/patches/esp-wifi-sys-s31-classic.patch"
apply_once "$hal_checkout" "$repo_root/patches/esp-radio-classic-s31.patch"
"$repo_root/scripts/apply-radio-affinity-fix.sh" "$hal_checkout"
"$repo_root/scripts/apply-fpu-context-fix.sh" "$hal_checkout"
printf '%s\n' 'Classic discovery patches applied; commit them in the dependency checkouts.'
printf '%s\n' 'Cargo.toml paths must match the selected checkout locations.'
