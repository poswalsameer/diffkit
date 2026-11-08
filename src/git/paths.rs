//! Conversion between the bytes Git uses for paths and native paths,
//! without loss on Unix and strictly as UTF-8 on Windows.

use super::errors::Result;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

/// Converts a path emitted by Git into a native path without loss.
///
/// On Unix, Git paths are arbitrary bytes and map directly onto `OsStr`.
/// Git for Windows stores and prints paths as UTF-8 and converts them to
/// UTF-16 itself, so on Windows the bytes are decoded strictly as UTF-8;
/// anything else is reported as malformed rather than silently replaced.
#[cfg(unix)]
pub fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
pub fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|e| super::errors::GitError::malformed("path is not valid UTF-8", e.to_string()))
}

/// The bytes Git uses for a native path: the inverse of [`path_from_git`].
#[cfg(unix)]
pub fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(path.as_os_str().as_bytes())
}

/// The bytes Git uses for a native path: the inverse of [`path_from_git`].
/// Paths produced by Diffkit are valid UTF-8 on Windows; any other path is
/// converted with replacement and can never match a Git path.
#[cfg(not(unix))]
pub fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    match path.to_string_lossy() {
        Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
        Cow::Owned(s) => Cow::Owned(s.into_bytes()),
    }
}
