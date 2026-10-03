//! Recursive FAT media catalog using short aliases as unambiguous paths.
extern crate alloc;
use alloc::{format, string::String, vec, vec::Vec};
use core::ops::ControlFlow;
use embedded_sdmmc::{BlockDevice, Mode, RawDirectory, RawFile, TimeSource, VolumeManager};
pub type Name = String;

/// At most three handles are live, regardless of directory depth. Parents
/// can close after opening a child because FAT directory handles are stateless.
fn directory<D: BlockDevice, T: TimeSource>(
    fs: &VolumeManager<D, T>,
    root: RawDirectory,
    path: &str,
) -> Result<RawDirectory, &'static str> {
    let mut current = root;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if part == "." || part == ".." {
            if current != root {
                let _ = fs.close_dir(current);
            }
            return Err("Invalid file path");
        }
        let next = fs.open_dir(current, part).map_err(|_| "Folder unavailable");
        if current != root {
            let _ = fs.close_dir(current);
        }
        current = next?;
    }
    Ok(current)
}
fn parent<'a, D: BlockDevice, T: TimeSource>(
    fs: &VolumeManager<D, T>,
    root: RawDirectory,
    path: &'a str,
) -> Result<(RawDirectory, &'a str), &'static str> {
    let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
    if file.is_empty() || matches!(file, "." | "..") {
        return Err("Invalid file path");
    }
    Ok((directory(fs, root, folder)?, file))
}
pub fn open<D: BlockDevice, T: TimeSource>(
    fs: &VolumeManager<D, T>,
    root: RawDirectory,
    path: &str,
    mode: Mode,
) -> Result<RawFile, &'static str> {
    let (dir, file) = parent(fs, root, path)?;
    let result = fs
        .open_file_in_dir(dir, file, mode)
        .map_err(|_| "Cannot open file");
    if dir != root {
        let _ = fs.close_dir(dir);
    }
    result
}
pub fn delete<D: BlockDevice, T: TimeSource>(
    fs: &VolumeManager<D, T>,
    root: RawDirectory,
    path: &str,
) -> Result<(), &'static str> {
    let (dir, file) = parent(fs, root, path)?;
    let result = (|| {
        let entry = fs
            .find_directory_entry(dir, file)
            .map_err(|_| "File no longer available")?;
        if entry.attributes.is_directory() || entry.attributes.is_volume() {
            return Err("Only files can be deleted");
        }
        fs.delete_entry_in_dir(dir, file)
            .map_err(|_| "Delete failed - RESCAN SD")
    })();
    if dir != root {
        let _ = fs.close_dir(dir);
    }
    result
}
pub fn catalog<D: BlockDevice, T: TimeSource>(
    fs: &VolumeManager<D, T>,
    root: RawDirectory,
) -> Result<Vec<Name>, &'static str> {
    let mut pending = vec![String::new()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let dir = directory(fs, root, &path)?;
        let result = fs
            .iterate_dir(dir, |entry| {
                if !entry.attributes.is_volume() {
                    let name = format!("{}", entry.name);
                    if !matches!(name.as_str(), "." | "..") {
                        let full = if path.is_empty() {
                            name
                        } else {
                            format!("{path}/{name}")
                        };
                        if entry.attributes.is_directory() {
                            pending.push(full);
                        } else {
                            files.push(full);
                        }
                    }
                }
                ControlFlow::Continue(())
            })
            .map_err(|_| "Directory read failed");
        if dir != root {
            let _ = fs.close_dir(dir);
        }
        result?;
    }
    files.sort_unstable();
    Ok(files)
}
