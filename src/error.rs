//! Errors a tool can answer with. Every message is written for the model: it says what was
//! wrong and, where there is one, what to do instead.

use std::io;

/// Why a folder operation was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    /// An absolute path was given.
    #[error("absolute paths are not allowed, use a path relative to the root folder: {0}")]
    Absolute(String),
    /// The path uses `..` or resolves (through a symlink) outside the root.
    #[error("path leaves the root folder and is refused: {0}")]
    Escape(String),
    /// Nothing exists at the path.
    #[error("not found: {0}")]
    NotFound(String),
    /// The target already exists.
    #[error("{0}")]
    Exists(String),
    /// A write or move on a read-only server.
    #[error(
        "the server is read-only: write and move are disabled (the operator starts it with --writable to allow them)"
    )]
    ReadOnly,
    /// The file or content is over the configured size limit.
    #[error("{path} is {size} bytes, over the {limit}-byte limit{hint}")]
    TooLarge {
        /// Root-relative path.
        path: String,
        /// Size found.
        size: u64,
        /// Configured limit.
        limit: u64,
        /// What to do instead (may be empty).
        hint: &'static str,
    },
    /// Bad arguments: wrong kind of entry, bad glob or regex, empty range.
    #[error("{0}")]
    Invalid(String),
    /// Any other I/O failure.
    #[error("{path}: {source}")]
    Io {
        /// Root-relative path.
        path: String,
        /// The underlying error.
        source: io::Error,
    },
}

impl FolderError {
    /// Classify an I/O error from `cap_std` for the path the model asked about.
    pub fn from_io(err: io::Error, path: &str) -> Self {
        // cap-std reports any resolution that leaves the root (absolute symlink target, `..`
        // through a symlink) as PermissionDenied "a path led outside of the filesystem".
        if err.kind() == io::ErrorKind::PermissionDenied
            && err.to_string().contains("outside of the filesystem")
        {
            return Self::Escape(path.to_string());
        }
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path.to_string()),
            io::ErrorKind::AlreadyExists => Self::Exists(format!("{path} already exists")),
            _ => Self::Io {
                path: path.to_string(),
                source: err,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_std_escape_maps_to_escape() {
        let e = io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a path led outside of the filesystem",
        );
        assert!(matches!(
            FolderError::from_io(e, "link"),
            FolderError::Escape(_)
        ));
    }

    #[test]
    fn not_found_and_exists_are_classified() {
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert!(matches!(
            FolderError::from_io(nf, "x"),
            FolderError::NotFound(_)
        ));
        let ex = io::Error::from(io::ErrorKind::AlreadyExists);
        assert!(
            FolderError::from_io(ex, "x")
                .to_string()
                .contains("already exists")
        );
    }
}
