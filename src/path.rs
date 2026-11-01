//! Root-relative path handling.
//!
//! Every path a tool receives is checked here before it reaches the filesystem: absolute paths
//! and any `..` component are refused with a clear message. This is the friendly first line;
//! the real confinement is [`cap_std::fs::Dir`], which refuses anything (including symlinks)
//! that resolves outside the root even if this check were bypassed.

use std::path::{Component, Path, PathBuf};

use crate::error::FolderError;

/// Turn a tool-supplied path into a clean path relative to the root folder.
///
/// `""` and `"."` mean the root itself and return an empty path. Leading `./` and repeated
/// separators are dropped. Absolute paths (`/etc`, `C:\x`, `\\server\share`) and any `..`
/// component are refused, even when the result would stay inside the root.
pub fn relative(input: &str) -> Result<PathBuf, FolderError> {
    let trimmed = input.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(FolderError::Absolute(input.to_string()));
    }
    let mut out = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(FolderError::Escape(input.to_string())),
            Component::RootDir | Component::Prefix(_) => {
                return Err(FolderError::Absolute(input.to_string()));
            }
        }
    }
    Ok(out)
}

/// Like [`relative`], but the root itself is refused: for tools that act on one entry
/// (`read`, `write`, `move`).
pub fn relative_entry(input: &str) -> Result<PathBuf, FolderError> {
    let rel = relative(input)?;
    if rel.as_os_str().is_empty() {
        return Err(FolderError::Invalid(
            "a file path is required; the root folder itself is not a file".into(),
        ));
    }
    Ok(rel)
}

/// The path to hand to `cap_std::fs::Dir`: `.` for the root, the relative path otherwise.
pub fn for_dir(rel: &Path) -> &Path {
    if rel.as_os_str().is_empty() {
        Path::new(".")
    } else {
        rel
    }
}

/// Root-relative path as shown to the model: `/` separators on every platform.
pub fn display(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_pass_and_are_normalised() {
        assert_eq!(relative("a/b.txt").unwrap(), PathBuf::from("a/b.txt"));
        assert_eq!(relative("./a//b.txt").unwrap(), PathBuf::from("a/b.txt"));
        assert_eq!(relative("").unwrap(), PathBuf::new());
        assert_eq!(relative(".").unwrap(), PathBuf::new());
        assert_eq!(relative(" notes.md ").unwrap(), PathBuf::from("notes.md"));
    }

    #[test]
    fn absolute_paths_are_refused() {
        for p in ["/etc/passwd", "/", "\\windows", "\\\\server\\share"] {
            assert!(
                matches!(relative(p), Err(FolderError::Absolute(_))),
                "{p} must be refused"
            );
        }
    }

    #[test]
    fn parent_components_are_refused_anywhere() {
        for p in ["..", "../x", "a/../../x", "a/../b", "a/b/.."] {
            assert!(
                matches!(relative(p), Err(FolderError::Escape(_))),
                "{p} must be refused"
            );
        }
    }

    #[test]
    fn entry_paths_must_name_something() {
        assert!(relative_entry(".").is_err());
        assert!(relative_entry("").is_err());
        assert!(relative_entry("a.txt").is_ok());
    }

    #[test]
    fn display_uses_forward_slashes() {
        assert_eq!(display(Path::new("a/b/c.rs")), "a/b/c.rs");
        assert_eq!(display(Path::new("")), ".");
        assert_eq!(for_dir(Path::new("")), Path::new("."));
    }
}
