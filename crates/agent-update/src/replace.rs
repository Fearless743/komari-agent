//! Atomic binary replacement.
//!
//! The implementation writes a temporary file next to the target, copies the
//! existing permissions, fsyncs it and renames it over the target. On Unix
//! `rename(2)` replaces the file atomically. Windows cannot rename over a
//! running executable, so it falls back to moving the old file aside first;
//! cleanup of that backup is best-effort and may be deferred until the next
//! run.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::UpdateError;

/// Replace `target` with `data`.
///
/// The target is never opened for writing: the new contents are written to a
/// sibling temporary file and renamed into place. Tests always pass a path in
/// a temporary directory, so the test process binary is never touched.
pub fn replace_file_atomically(target: &Path, data: &[u8]) -> Result<(), UpdateError> {
    let parent = parent_directory(target);

    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|source| UpdateError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let temporary = create_temporary_file(parent, target, data)?;

    let result = rename_over(&temporary, target).map_err(|source| UpdateError::Io {
        path: target.to_path_buf(),
        source,
    });

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }

    // Best-effort directory fsync so the rename survives a power loss.
    #[cfg(unix)]
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }

    Ok(())
}

fn parent_directory(target: &Path) -> &Path {
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temporary_file(
    parent: &Path,
    target: &Path,
    data: &[u8],
) -> Result<PathBuf, UpdateError> {
    let mut last_error = None;

    for _ in 0..8 {
        let path = temporary_path(parent, target);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);

        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some((path, source));
                continue;
            }
            Err(source) => {
                return Err(UpdateError::Io { path, source });
            }
        };

        let result = (|| -> io::Result<()> {
            file.write_all(data)?;
            file.flush()?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(source) = result {
            let _ = fs::remove_file(&path);
            return Err(UpdateError::Io { path, source });
        }

        drop(file);

        if let Err(source) = copy_permissions(target, &path) {
            let _ = fs::remove_file(&path);
            return Err(UpdateError::Io { path, source });
        }

        return Ok(path);
    }

    let (path, source) = last_error.unwrap_or_else(|| {
        (
            parent.join("komari-agent.tmp"),
            io::Error::new(io::ErrorKind::AlreadyExists, "temporary name collision"),
        )
    });
    Err(UpdateError::Io { path, source })
}

fn temporary_path(parent: &Path, target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "komari-agent".to_owned());
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    parent.join(format!(
        ".{name}.new-{}-{nanos}-{counter}",
        std::process::id()
    ))
}

fn copy_permissions(target: &Path, temporary: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(target)
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .unwrap_or(0o755);

        fs::set_permissions(temporary, fs::Permissions::from_mode(mode))
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(target)
            .map(|metadata| metadata.permissions())
            .or_else(|_| fs::metadata(temporary).map(|metadata| metadata.permissions()))?;
        permissions.set_readonly(false);
        fs::set_permissions(temporary, permissions)
    }
}

#[cfg(not(windows))]
fn rename_over(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn rename_over(temporary: &Path, target: &Path) -> io::Result<()> {
    if fs::rename(temporary, target).is_ok() {
        return Ok(());
    }

    // Windows rejects renaming over an existing file. Move the old binary
    // aside, install the new one and try to delete the backup. If the old
    // binary is the currently running image, deletion fails and the backup is
    // left behind for cleanup on a later run.
    let backup = target.with_file_name(format!(
        ".{}.old-{}",
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "komari-agent".to_owned()),
        std::process::id()
    ));

    let _ = fs::remove_file(&backup);
    fs::rename(target, &backup)?;

    match fs::rename(temporary, target) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&backup, target);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn replaces_target_and_preserves_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("komari-agent");
        fs::write(&target, b"old binary").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
        }

        replace_file_atomically(&target, b"new binary").unwrap();

        let mut contents = Vec::new();
        File::open(&target)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();
        assert_eq!(contents, b"new binary");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o750);
        }

        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".new-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary file left behind");
    }

    #[test]
    fn creates_missing_target_with_executable_mode() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("sub").join("komari-agent");

        replace_file_atomically(&target, b"binary").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"binary");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn failed_replace_keeps_target_and_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target-directory");
        fs::create_dir(&target).unwrap();

        let error = replace_file_atomically(&target, b"new binary").unwrap_err();
        assert!(matches!(error, UpdateError::Io { .. }));
        assert!(target.is_dir());

        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".new-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary file left behind");
    }
}
