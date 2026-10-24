//! Private, atomic file writes and bounded reads (Unix: Linux and macOS,
//! ADR-0004).

use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Mode for files holding secrets.
pub(crate) const FILE_MODE: u32 = 0o600;
/// Mode for directories Farsight creates.
pub(crate) const DIR_MODE: u32 = 0o700;

/// A failed filesystem step, with what was being done and to which path.
#[derive(Debug)]
pub(crate) struct FsError {
    pub(crate) op: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) source: io::Error,
}

fn at(op: &'static str, path: &Path) -> impl FnOnce(io::Error) -> FsError {
    let path = path.to_path_buf();
    move |source| FsError { op, path, source }
}

/// Replaces `path` with `bytes` atomically.
///
/// Missing parent directories are created with mode 0700 (existing ones are
/// left alone). The data goes to a 0600 temporary file in the same
/// directory, is flushed to disk, renamed over `path`, and then the
/// directory is flushed so the rename itself is durable. Readers see either
/// the old file or the new one, never a mix.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), FsError> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .map_err(at("create directory", dir))?;

    let file_name = path
        .file_name()
        .map_or_else(|| "farsight".into(), |n| n.to_string_lossy().into_owned());
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .permissions(Permissions::from_mode(FILE_MODE))
        .tempfile_in(dir)
        .map_err(at("create a temporary file in", dir))?;
    // Enforce the mode regardless of the process umask.
    temp.as_file()
        .set_permissions(Permissions::from_mode(FILE_MODE))
        .map_err(at("set permissions on", temp.path()))?;
    temp.write_all(bytes).map_err(at("write", temp.path()))?;
    temp.as_file()
        .sync_all()
        .map_err(at("flush", temp.path()))?;
    temp.persist(path).map_err(|e| FsError {
        op: "replace",
        path: path.to_path_buf(),
        source: e.error,
    })?;
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(at("flush directory", dir))
}

/// Why a bounded read failed.
#[derive(Debug)]
pub(crate) enum ReadError {
    Io(FsError),
    TooLarge(u64),
}

/// Reads a whole file, refusing anything over `max` bytes.
pub(crate) fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, ReadError> {
    let file = File::open(path).map_err(|e| ReadError::Io(at("read", path)(e)))?;
    let len = file
        .metadata()
        .map_err(|e| ReadError::Io(at("read", path)(e)))?
        .len();
    if len > max {
        return Err(ReadError::TooLarge(len));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    // `take` guards against a file that grows while we read it.
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| ReadError::Io(at("read", path)(e)))?;
    let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if read > max {
        return Err(ReadError::TooLarge(read));
    }
    Ok(bytes)
}

/// Whether `path` exists (following symlinks); errors count as "exists" so
/// that callers never overwrite something they could not inspect.
pub(crate) fn exists(path: &Path) -> bool {
    !matches!(fs::metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}
