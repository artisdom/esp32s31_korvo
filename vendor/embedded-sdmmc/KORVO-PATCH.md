# Korvo deletion patch

Vendored from crates.io `embedded-sdmmc` 0.10.0 (MIT OR Apache-2.0).
Source, package metadata, README, changelog and upstream v0.10.0 license texts
are retained; unused example/test targets and development dependencies are
omitted. Both the firmware and media host tests use this local copy.

Upstream `delete_entry_in_dir` marks only the short directory entry deleted.
For this app deletion also removes the preceding LFN slots across directory
sector/cluster boundaries, frees every cluster including the head, updates
both FAT copies through the existing writer, and updates FAT32 FSInfo.

Names are unlinked before freeing storage. Interrupted writes may leave lost
allocation; deletion is not atomic and does not provide power-loss recovery.
The existing open-file/empty-directory checks remain in place. The app limits
its deletion UI to regular root files and blocks deletion during audio activity.

Validation: `tests/media-host` exercises FAT16/FAT32, empty/single/multiple
cluster files, a sector-spanning LFN, refusal to delete an open file, preservation
of adjacent files and the boot sector, and read-only `fsck.fat -n` afterward.
