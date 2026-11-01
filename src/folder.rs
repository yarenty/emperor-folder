//! The root folder and the six operations on it.
//!
//! [`Folder`] holds the root opened as a [`cap_std::fs::Dir`]. Every file operation goes
//! through that handle with a root-relative path, so the operating system (via cap-std) refuses
//! anything that would resolve outside the root: absolute paths, `..`, and symlinks pointing
//! out. Listing and search walk with the [`ignore`] crate (ripgrep's walker, `.gitignore` and
//! hidden-file aware, symlinks not followed); each path the walker yields is made relative to
//! the root and re-opened through the `Dir`, never by its absolute path, so an entry swapped for
//! an escaping symlink mid-walk is skipped rather than read.
//!
//! The operations are synchronous; the MCP layer runs them on the blocking pool.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::FolderError;
use crate::path;

/// Default for [`Config::max_file_bytes`]: 1 MiB.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Default for [`Config::max_results`].
pub const DEFAULT_MAX_RESULTS: usize = 200;
/// Bytes inspected for a NUL byte to decide a file is binary (ripgrep's heuristic).
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Longest matched line returned by `search`, in characters.
const MAX_MATCH_LINE_CHARS: usize = 240;
/// Compiled-regex size cap, so a pathological pattern cannot eat memory.
const REGEX_SIZE_LIMIT: usize = 1024 * 1024;

/// What the operator configured at start-up.
#[derive(Debug, Clone)]
pub struct Config {
    /// The folder the server is confined to.
    pub root: PathBuf,
    /// Allow `write` and `move`. Off by default.
    pub writable: bool,
    /// Include hidden files and directories in `list` and `search`.
    pub hidden: bool,
    /// Ignore `.gitignore`, `.ignore` and git exclude files in `list` and `search`.
    pub no_ignore: bool,
    /// Largest file `read`, `search` and `stat` will open, and largest `write`.
    pub max_file_bytes: u64,
    /// Most entries `list` and matches `search` return.
    pub max_results: usize,
}

impl Config {
    /// Read-only defaults for `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writable: false,
            hidden: false,
            no_ignore: false,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_results: DEFAULT_MAX_RESULTS,
        }
    }

    /// Allow or forbid `write` and `move`.
    pub fn writable(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }
}

/// How `write` treats an existing file.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum WriteMode {
    /// Create a new file; refuse if it exists.
    #[default]
    Create,
    /// Replace the file's content, creating it if missing.
    Overwrite,
    /// Add to the end of the file, creating it if missing.
    Append,
}

/// Result of `read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutput {
    /// Text content.
    Text {
        /// Root-relative path.
        path: String,
        /// The lines read, newlines kept.
        content: String,
        /// First line returned (1-based); 0 when nothing was returned.
        first_line: usize,
        /// Last line returned (1-based); 0 when nothing was returned.
        last_line: usize,
        /// Total lines in the file, when the read reached the end.
        total_lines: Option<usize>,
        /// The byte limit cut the content short.
        truncated: bool,
    },
    /// A binary file: reported, not dumped.
    Binary {
        /// Root-relative path.
        path: String,
        /// Size in bytes.
        size: u64,
    },
}

/// The root folder, opened once, and the operator's limits.
#[derive(Debug)]
pub struct Folder {
    dir: Dir,
    root: PathBuf,
    config: Config,
}

impl Folder {
    /// Open the configured root. Fails when it does not exist or is not a directory.
    pub fn open(config: Config) -> Result<Self, FolderError> {
        let shown = config.root.display().to_string();
        let root = config
            .root
            .canonicalize()
            .map_err(|e| FolderError::from_io(e, &shown))?;
        if !root.is_dir() {
            return Err(FolderError::Invalid(format!(
                "root {shown} is not a directory"
            )));
        }
        let dir = Dir::open_ambient_dir(&root, ambient_authority())
            .map_err(|e| FolderError::from_io(e, &shown))?;
        Ok(Self { dir, root, config })
    }

    /// The canonical root path (for logs and `doctor`; never used to open files).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The configuration in force.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The root as a capability handle.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// `list`: entries under `path` (the root when empty), optionally filtered by a glob on the
    /// root-relative path, one level deep unless `recursive`.
    pub fn list(
        &self,
        path: &str,
        glob: Option<&str>,
        recursive: bool,
        max_entries: Option<usize>,
    ) -> Result<Value, FolderError> {
        let rel = path::relative(path)?;
        self.require_dir(&rel, path)?;
        let matcher = glob.map(compile_glob).transpose()?;
        let limit = self.cap(max_entries);

        let mut walker = self.walker(&rel);
        if !recursive {
            walker.max_depth(Some(1));
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in walker.build().flatten() {
            if entry.depth() == 0 {
                continue;
            }
            let Some(entry_rel) = self.to_relative(entry.path()) else {
                continue;
            };
            if let Some(m) = &matcher
                && !m.is_match(&entry_rel)
            {
                continue;
            }
            // Re-open through the capability handle; anything it refuses is not listed.
            let Ok(md) = self.dir.symlink_metadata(&entry_rel) else {
                continue;
            };
            if entries.len() == limit {
                truncated = true;
                break;
            }
            let mut item = json!({ "path": path::display(&entry_rel), "kind": kind(&md) });
            if md.is_file() {
                item["size"] = json!(md.len());
            }
            entries.push(item);
        }
        Ok(json!({
            "path": path::display(&rel),
            "entries": entries,
            "truncated": truncated,
        }))
    }

    /// `read`: a text file, whole or lines `start_line..=end_line` (1-based).
    ///
    /// A whole-file read is refused above the size limit; a line-range read streams the file and
    /// stops at `end_line`, returning at most the size limit in bytes.
    pub fn read(
        &self,
        path: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<ReadOutput, FolderError> {
        let rel = path::relative_entry(path)?;
        let shown = path::display(&rel);
        let md = self
            .dir
            .metadata(&rel)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        if md.is_dir() {
            return Err(FolderError::Invalid(format!(
                "{shown} is a directory; use list"
            )));
        }
        let limit = self.config.max_file_bytes;
        let ranged = start_line.is_some() || end_line.is_some();
        if !ranged && md.len() > limit {
            return Err(FolderError::TooLarge {
                path: shown,
                size: md.len(),
                limit,
                hint: "; read a line range with start_line and end_line",
            });
        }
        let start = start_line.unwrap_or(1).max(1);
        let end = end_line.unwrap_or(usize::MAX);
        if end < start {
            return Err(FolderError::Invalid(format!(
                "end_line {end} is before start_line {start}"
            )));
        }

        let file = self
            .dir
            .open(&rel)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let head = reader
            .fill_buf()
            .map_err(|e| FolderError::from_io(e, &shown))?;
        if is_binary(head) {
            return Ok(ReadOutput::Binary {
                path: shown,
                size: md.len(),
            });
        }

        let io = |e| FolderError::from_io(e, &shown);
        let mut out: Vec<u8> = Vec::new();
        let mut line = 0usize; // lines fully consumed
        let mut first = 0usize;
        let mut truncated = false;
        let mut at_eof = false;
        while line < end {
            if line + 1 < start {
                if reader.skip_until(b'\n').map_err(io)? == 0 {
                    at_eof = true;
                    break;
                }
                line += 1;
                continue;
            }
            let budget = limit - out.len() as u64;
            let n = (&mut reader)
                .take(budget + 1)
                .read_until(b'\n', &mut out)
                .map_err(io)?;
            if n == 0 {
                at_eof = true;
                break;
            }
            line += 1;
            if first == 0 {
                first = line;
            }
            if out.len() as u64 > limit {
                out.truncate(limit as usize);
                truncated = true;
                break;
            }
        }
        if !at_eof && !truncated {
            // Stopped at end_line: the file is complete if nothing follows.
            at_eof = reader.fill_buf().map_err(io)?.is_empty();
        }
        let last = if first == 0 { 0 } else { line };
        Ok(ReadOutput::Text {
            path: shown,
            content: String::from_utf8_lossy(&out).into_owned(),
            first_line: first,
            last_line: last,
            total_lines: (at_eof && !truncated).then_some(line),
            truncated,
        })
    }

    /// `search`: lines matching `pattern` in text files under `path` (or in that one file).
    pub fn search(
        &self,
        pattern: &str,
        path: &str,
        glob: Option<&str>,
        ignore_case: bool,
    ) -> Result<Value, FolderError> {
        let re = RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| FolderError::Invalid(format!("invalid regex: {e}")))?;
        let rel = path::relative(path)?;
        let shown = path::display(&rel);
        let md = self
            .dir
            .metadata(path::for_dir(&rel))
            .map_err(|e| FolderError::from_io(e, &shown))?;
        let matcher = glob.map(compile_glob).transpose()?;
        let limit = self.config.max_results;

        let files: Vec<PathBuf> = if md.is_file() {
            vec![rel.clone()]
        } else {
            self.walker(&rel)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .filter_map(|e| self.to_relative(e.path()))
                .filter(|r| matcher.as_ref().is_none_or(|m| m.is_match(r)))
                .collect()
        };

        let mut matches = Vec::new();
        let (mut searched, mut skipped_binary, mut skipped_large) = (0usize, 0usize, 0usize);
        let mut truncated = false;
        'files: for file_rel in files {
            let Ok(fmd) = self.dir.symlink_metadata(&file_rel) else {
                continue;
            };
            if !fmd.is_file() {
                continue;
            }
            if fmd.len() > self.config.max_file_bytes {
                skipped_large += 1;
                continue;
            }
            let Ok(bytes) = self.dir.read(&file_rel) else {
                continue;
            };
            if is_binary(&bytes) {
                skipped_binary += 1;
                continue;
            }
            searched += 1;
            let text = String::from_utf8_lossy(&bytes);
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if matches.len() == limit {
                        truncated = true;
                        break 'files;
                    }
                    matches.push(json!({
                        "path": path::display(&file_rel),
                        "line": i + 1,
                        "text": clip(line, MAX_MATCH_LINE_CHARS),
                    }));
                }
            }
        }
        Ok(json!({
            "pattern": pattern,
            "matches": matches,
            "files_searched": searched,
            "skipped_binary": skipped_binary,
            "skipped_too_large": skipped_large,
            "truncated": truncated,
        }))
    }

    /// `write`: put `content` at `path`, creating parent directories.
    pub fn write(&self, path: &str, content: &str, mode: WriteMode) -> Result<Value, FolderError> {
        if !self.config.writable {
            return Err(FolderError::ReadOnly);
        }
        let rel = path::relative_entry(path)?;
        let shown = path::display(&rel);
        let size = content.len() as u64;
        if size > self.config.max_file_bytes {
            return Err(FolderError::TooLarge {
                path: shown,
                size,
                limit: self.config.max_file_bytes,
                hint: "",
            });
        }
        self.create_parent(&rel, &shown)?;
        let mut opts = OpenOptions::new();
        match mode {
            WriteMode::Create => opts.write(true).create_new(true),
            WriteMode::Overwrite => opts.write(true).create(true).truncate(true),
            WriteMode::Append => opts.append(true).create(true),
        };
        let mut file =
            self.dir
                .open_with(&rel, &opts)
                .map_err(|e| match FolderError::from_io(e, &shown) {
                    FolderError::Exists(_) => FolderError::Exists(format!(
                        "{shown} already exists; use mode overwrite or append"
                    )),
                    other => other,
                })?;
        file.write_all(content.as_bytes())
            .map_err(|e| FolderError::from_io(e, &shown))?;
        Ok(json!({ "path": shown, "mode": mode, "bytes_written": size }))
    }

    /// `move`: rename `from` to `to` inside the root; refuses to replace `to` unless `overwrite`.
    pub fn move_entry(&self, from: &str, to: &str, overwrite: bool) -> Result<Value, FolderError> {
        if !self.config.writable {
            return Err(FolderError::ReadOnly);
        }
        let from_rel = path::relative_entry(from)?;
        let to_rel = path::relative_entry(to)?;
        let (from_shown, to_shown) = (path::display(&from_rel), path::display(&to_rel));
        self.dir
            .symlink_metadata(&from_rel)
            .map_err(|e| FolderError::from_io(e, &from_shown))?;
        if from_rel == to_rel {
            return Err(FolderError::Invalid(format!(
                "{from_shown} and {to_shown} are the same path"
            )));
        }
        if to_rel.starts_with(&from_rel) {
            return Err(FolderError::Invalid(format!(
                "cannot move {from_shown} into itself"
            )));
        }
        if !overwrite && self.dir.symlink_metadata(&to_rel).is_ok() {
            return Err(FolderError::Exists(format!(
                "{to_shown} already exists; pass overwrite: true to replace it"
            )));
        }
        self.create_parent(&to_rel, &to_shown)?;
        self.dir
            .rename(&from_rel, &self.dir, &to_rel)
            .map_err(|e| FolderError::from_io(e, &from_shown))?;
        Ok(json!({ "from": from_shown, "to": to_shown }))
    }

    /// `stat`: kind, size, modified time, and line count for a text file within the size limit.
    pub fn stat(&self, path: &str) -> Result<Value, FolderError> {
        let rel = path::relative(path)?;
        let shown = path::display(&rel);
        let md = self
            .dir
            .symlink_metadata(path::for_dir(&rel))
            .map_err(|e| FolderError::from_io(e, &shown))?;
        let mut out = json!({
            "path": shown,
            "kind": kind(&md),
            "size": md.len(),
            "readonly": md.permissions().readonly(),
        });
        if let Some(secs) = md
            .modified()
            .ok()
            .and_then(|t| t.into_std().duration_since(UNIX_EPOCH).ok())
        {
            out["modified"] = json!(rfc3339_utc(secs.as_secs()));
        }
        if md.is_file() && md.len() <= self.config.max_file_bytes {
            let bytes = self
                .dir
                .read(&rel)
                .map_err(|e| FolderError::from_io(e, &shown))?;
            if is_binary(&bytes) {
                out["binary"] = json!(true);
            } else {
                out["binary"] = json!(false);
                out["lines"] = json!(count_lines(&bytes));
            }
        }
        Ok(out)
    }

    /// Number of files under the root the walker sees, stopping once `stop_after` is passed.
    pub fn count_files(&self, stop_after: usize) -> usize {
        self.walker(Path::new(""))
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .take(stop_after.saturating_add(1))
            .count()
    }

    /* --- helpers ------------------------------------------------------------------------------ */

    fn cap(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.config.max_results)
            .clamp(1, self.config.max_results.max(1))
    }

    fn require_dir(&self, rel: &Path, asked: &str) -> Result<(), FolderError> {
        let md = self
            .dir
            .metadata(path::for_dir(rel))
            .map_err(|e| FolderError::from_io(e, asked))?;
        if md.is_dir() {
            Ok(())
        } else {
            Err(FolderError::Invalid(format!(
                "{} is not a directory; use read or stat",
                path::display(rel)
            )))
        }
    }

    fn create_parent(&self, rel: &Path, shown: &str) -> Result<(), FolderError> {
        if let Some(parent) = rel.parent()
            && !parent.as_os_str().is_empty()
        {
            match self.dir.create_dir_all(parent) {
                Ok(()) => {}
                // cap-std answers "exists" for a symlink in the way; resolve it to say whether
                // it is a usable directory or a link out of the root.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    match self.dir.metadata(parent) {
                        Ok(md) if md.is_dir() => {}
                        Ok(_) => {
                            return Err(FolderError::Invalid(format!(
                                "{} is not a directory",
                                path::display(parent)
                            )));
                        }
                        Err(e) => return Err(FolderError::from_io(e, shown)),
                    }
                }
                Err(e) => return Err(FolderError::from_io(e, shown)),
            }
        }
        Ok(())
    }

    /// A walker over `rel` (already checked to be a directory through the `Dir`).
    fn walker(&self, rel: &Path) -> WalkBuilder {
        let mut b = WalkBuilder::new(self.root.join(rel));
        let honour_ignore = !self.config.no_ignore;
        b.hidden(!self.config.hidden)
            .ignore(honour_ignore)
            .git_ignore(honour_ignore)
            .git_global(false)
            .git_exclude(honour_ignore)
            .parents(honour_ignore)
            // .gitignore applies even when the root is not a git checkout.
            .require_git(false)
            .follow_links(false)
            .sort_by_file_name(|a, b| a.cmp(b));
        b
    }

    /// The walker's absolute path made relative to the root; `None` if it is not under it.
    fn to_relative(&self, abs: &Path) -> Option<PathBuf> {
        let rel = abs.strip_prefix(&self.root).ok()?;
        // Re-validate: the walker never produces `..`, but nothing reaches the Dir unchecked.
        path::relative(rel.to_str()?).ok()
    }
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, FolderError> {
    GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| FolderError::Invalid(format!("invalid glob {pattern:?}: {e}")))
}

fn kind(md: &Metadata) -> &'static str {
    let ft = md.file_type();
    if ft.is_symlink() {
        "symlink"
    } else if ft.is_dir() {
        "dir"
    } else if ft.is_file() {
        "file"
    } else {
        "other"
    }
}

/// A NUL byte in the first 8 KiB means binary.
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

fn count_lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    if bytes.last().is_some_and(|&b| b != b'\n') {
        newlines + 1
    } else {
        newlines
    }
}

fn clip(line: &str, max_chars: usize) -> String {
    match line.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &line[..idx]),
        None => line.to_string(),
    }
}

/// Seconds since the Unix epoch as an RFC 3339 UTC timestamp, without a date crate.
fn rfc3339_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn line_counting() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
    }

    #[test]
    fn binary_sniff_and_clip() {
        assert!(is_binary(b"ab\0cd"));
        assert!(!is_binary(b"plain text"));
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("żółw", 10), "żółw");
    }

    #[test]
    fn glob_matches_root_relative_paths() {
        let m = compile_glob("*.rs").unwrap();
        assert!(m.is_match("src/main.rs"));
        assert!(!m.is_match("README.md"));
        assert!(compile_glob("a[").is_err());
    }
}
