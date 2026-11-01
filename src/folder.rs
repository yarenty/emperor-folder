//! The root folders and the seven operations on them.
//!
//! [`Folder`] holds each root opened as a [`cap_std::fs::Dir`]. Every file operation goes
//! through one of those handles with a root-relative path, so the operating system (via cap-std)
//! refuses anything that would resolve outside that root: absolute paths, `..`, and symlinks
//! pointing out, including symlinks from one root into another. Listing and search walk with the
//! [`ignore`] crate (ripgrep's walker, `.gitignore` and hidden-file aware, symlinks not
//! followed); each path the walker yields is made relative to its root and re-opened through the
//! `Dir`, never by its absolute path, so an entry swapped for an escaping symlink mid-walk is
//! skipped rather than read.
//!
//! With one root, tool paths are plain root-relative paths. With several, a tool path starts
//! with the root's name (`docs/guide.md`), and `.` is a virtual folder holding the roots.
//!
//! The operations are synchronous; the MCP layer runs them on the blocking pool.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::UNIX_EPOCH;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use emperor_mcp::limits::{self, Limits};
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
/// Default for [`Config::max_chars`]: emperor-mcp's default output cap.
pub const DEFAULT_MAX_CHARS: usize = limits::DEFAULT_MAX_CHARS;
/// Largest image `read` returns as image content: 5 MiB.
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// Bytes inspected for a NUL byte to decide a file is binary (ripgrep's heuristic).
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Longest matched line returned by `search`, in characters.
const MAX_MATCH_LINE_CHARS: usize = 240;
/// Compiled-regex size cap, so a pathological pattern cannot eat memory.
const REGEX_SIZE_LIMIT: usize = 1024 * 1024;

/// One root folder as the operator named it: `DIR` or `NAME=DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootSpec {
    /// The name paths start with when there are several roots; `None` uses the folder's
    /// basename.
    pub name: Option<String>,
    /// The folder.
    pub path: PathBuf,
}

impl RootSpec {
    /// An unnamed root (named after its folder).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            name: None,
            path: path.into(),
        }
    }

    /// A named root.
    pub fn named(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: Some(name.into()),
            path: path.into(),
        }
    }
}

impl FromStr for RootSpec {
    type Err = String;

    /// `DIR`, or `NAME=DIR` when the part before the first `=` contains no path separator.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("an empty root".into());
        }
        if let Some((name, dir)) = s.split_once('=')
            && !name.is_empty()
            && !name.contains(['/', '\\'])
        {
            if dir.is_empty() {
                return Err(format!("root {name}= names no folder"));
            }
            return Ok(Self::named(name, dir));
        }
        Ok(Self::new(s))
    }
}

/// What the operator configured at start-up.
#[derive(Debug, Clone)]
pub struct Config {
    /// The folders the server is confined to (at least one).
    pub roots: Vec<RootSpec>,
    /// Allow `write`, `edit` and `move`. Off by default.
    pub writable: bool,
    /// Include hidden files and directories in `list` and `search`.
    pub hidden: bool,
    /// Ignore `.gitignore`, `.ignore` and git exclude files in `list` and `search`.
    pub no_ignore: bool,
    /// Largest file `read`, `search`, `stat` and `edit` will open, and largest `write`.
    pub max_file_bytes: u64,
    /// Most entries `list` and matches `search` return.
    pub max_results: usize,
    /// Most characters of text one `read` returns.
    pub max_chars: usize,
}

impl Config {
    /// Read-only defaults for one unnamed `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_roots(vec![RootSpec::new(root)])
    }

    /// Read-only defaults for several roots.
    pub fn with_roots(roots: Vec<RootSpec>) -> Self {
        Self {
            roots,
            writable: false,
            hidden: false,
            no_ignore: false,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_results: DEFAULT_MAX_RESULTS,
            max_chars: DEFAULT_MAX_CHARS,
        }
    }

    /// Add a named root.
    pub fn root(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.roots.push(RootSpec::named(name, path));
        self
    }

    /// Allow or forbid `write`, `edit` and `move`.
    pub fn writable(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }

    /// Set the character cap of `read`, clamped to emperor-mcp's ceiling.
    pub fn max_chars(mut self, n: usize) -> Self {
        self.max_chars = Limits::default().with_max_chars(n).max_chars();
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
        /// Path as the tools address it.
        path: String,
        /// The lines read, newlines kept.
        content: String,
        /// First line returned (1-based); 0 when nothing was returned.
        first_line: usize,
        /// Last line returned (1-based); 0 when nothing was returned.
        last_line: usize,
        /// Total lines in the file, when the read reached the end.
        total_lines: Option<usize>,
        /// The byte or character limit cut the content short.
        truncated: bool,
        /// The cut fell inside `last_line` (it is incomplete).
        partial_last_line: bool,
    },
    /// A small PNG, JPEG, GIF or WebP image.
    Image {
        /// Path as the tools address it.
        path: String,
        /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
        mime: &'static str,
        /// The file's bytes.
        data: Vec<u8>,
    },
    /// A binary file: reported, not dumped.
    Binary {
        /// Path as the tools address it.
        path: String,
        /// Size in bytes.
        size: u64,
    },
}

/// One opened root.
#[derive(Debug)]
struct Root {
    name: String,
    dir: Dir,
    path: PathBuf,
    /// Several roots: tool paths carry the name as their first component.
    prefixed: bool,
}

impl Root {
    /// A root-relative path as the tools address it.
    fn show(&self, rel: &Path) -> String {
        if !self.prefixed {
            return path::display(rel);
        }
        if rel.as_os_str().is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.name, path::display(rel))
        }
    }

    /// The walker's absolute path made relative to this root; `None` if it is not under it.
    fn to_relative(&self, abs: &Path) -> Option<PathBuf> {
        let rel = abs.strip_prefix(&self.path).ok()?;
        // Re-validate: the walker never produces `..`, but nothing reaches the Dir unchecked.
        path::relative(rel.to_str()?).ok()
    }
}

/// A tool path resolved to a root and a path inside it, or the virtual top of several roots.
enum Target<'a> {
    Top,
    In(&'a Root, PathBuf),
}

/// The root folders, opened once, and the operator's limits.
#[derive(Debug)]
pub struct Folder {
    roots: Vec<Root>,
    config: Config,
}

impl Folder {
    /// Open the configured roots. Fails when one does not exist or is not a directory, when two
    /// roots share a name, or when a name is not a single plain path component.
    pub fn open(config: Config) -> Result<Self, FolderError> {
        if config.roots.is_empty() {
            return Err(FolderError::Invalid("no root folder configured".into()));
        }
        let prefixed = config.roots.len() > 1;
        let mut roots: Vec<Root> = Vec::with_capacity(config.roots.len());
        for spec in &config.roots {
            let shown = spec.path.display().to_string();
            let canonical = spec
                .path
                .canonicalize()
                .map_err(|e| FolderError::from_io(e, &shown))?;
            if !canonical.is_dir() {
                return Err(FolderError::Invalid(format!(
                    "root {shown} is not a directory"
                )));
            }
            let name = match &spec.name {
                Some(n) => n.clone(),
                None => canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "root".into()),
            };
            if !valid_root_name(&name) {
                return Err(FolderError::Invalid(format!(
                    "root name {name:?} must be one plain path component (no /, \\, . or ..)"
                )));
            }
            if prefixed && roots.iter().any(|r| r.name == name) {
                return Err(FolderError::Invalid(format!(
                    "two roots are named {name:?}; name them with --root NAME=DIR"
                )));
            }
            let dir = Dir::open_ambient_dir(&canonical, ambient_authority())
                .map_err(|e| FolderError::from_io(e, &shown))?;
            roots.push(Root {
                name,
                dir,
                path: canonical,
                prefixed,
            });
        }
        Ok(Self { roots, config })
    }

    /// Each root's name and canonical path (for logs and `doctor`; never used to open files).
    pub fn roots(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.roots
            .iter()
            .map(|r| (r.name.as_str(), r.path.as_path()))
    }

    /// The configuration in force.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// `list`: entries under `path` (the root when empty), optionally filtered by a glob on the
    /// path as the tools address it, one level deep unless `recursive`.
    pub fn list(
        &self,
        path: &str,
        glob: Option<&str>,
        recursive: bool,
        max_entries: Option<usize>,
    ) -> Result<Value, FolderError> {
        let target = self.resolve(path)?;
        let matcher = glob.map(compile_glob).transpose()?;
        let limit = self.cap(max_entries);
        let mut entries = Vec::new();
        let mut truncated = false;
        let shown = match target {
            Target::In(root, rel) => {
                self.require_dir(root, &rel, path)?;
                truncated =
                    self.list_in(root, &rel, matcher.as_ref(), recursive, limit, &mut entries);
                root.show(&rel)
            }
            Target::Top => {
                for root in &self.roots {
                    if matcher.as_ref().is_none_or(|m| m.is_match(&root.name)) {
                        if entries.len() == limit {
                            truncated = true;
                            break;
                        }
                        entries.push(json!({ "path": root.name, "kind": "dir" }));
                    }
                    if recursive
                        && self.list_in(
                            root,
                            Path::new(""),
                            matcher.as_ref(),
                            true,
                            limit,
                            &mut entries,
                        )
                    {
                        truncated = true;
                        break;
                    }
                }
                ".".to_string()
            }
        };
        Ok(json!({
            "path": shown,
            "entries": entries,
            "truncated": truncated,
        }))
    }

    /// `read`: a text file, whole or lines `start_line..=end_line` (1-based), or a small image.
    ///
    /// A whole-file read is refused above the size limit; a line-range read streams the file and
    /// stops at `end_line`. Either returns at most the byte limit and the character limit.
    pub fn read(
        &self,
        path: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<ReadOutput, FolderError> {
        let (root, rel) = self.resolve_entry(path)?;
        let shown = root.show(&rel);
        let md = root
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

        let file = root
            .dir
            .open(&rel)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        let io = |e| FolderError::from_io(e, &shown);
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let head = reader.fill_buf().map_err(io)?;
        if let Some(mime) = image_mime(head)
            && !ranged
            && md.len() <= MAX_IMAGE_BYTES
        {
            let mut data = Vec::with_capacity(md.len() as usize);
            reader
                .take(limit.min(MAX_IMAGE_BYTES) + 1)
                .read_to_end(&mut data)
                .map_err(io)?;
            if data.len() as u64 <= limit.min(MAX_IMAGE_BYTES) {
                return Ok(ReadOutput::Image {
                    path: shown,
                    mime,
                    data,
                });
            }
            // Grew past the limit while being read: report it like any binary file.
            return Ok(ReadOutput::Binary {
                path: shown,
                size: data.len() as u64,
            });
        }
        if is_binary(head) {
            return Ok(ReadOutput::Binary {
                path: shown,
                size: md.len(),
            });
        }

        let max_chars = self.config.max_chars.max(1);
        let mut out: Vec<u8> = Vec::new();
        let mut chars = 0usize;
        let mut line = 0usize; // lines fully consumed
        let mut first = 0usize;
        let mut truncated = false;
        let mut partial = false;
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
            let before = out.len();
            let budget = limit - before as u64;
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
                partial = true;
                break;
            }
            let line_chars = count_chars(&out[before..]);
            if chars + line_chars > max_chars {
                truncated = true;
                if line == first {
                    // One line longer than the whole budget: return its start.
                    let keep = byte_index_of_char(&out[before..], max_chars - chars);
                    out.truncate(before + keep);
                    partial = true;
                } else {
                    out.truncate(before);
                    line -= 1;
                }
                break;
            }
            chars += line_chars;
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
            partial_last_line: partial,
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
        let matcher = glob.map(compile_glob).transpose()?;
        let limit = self.config.max_results;

        let mut files: Vec<(&Root, PathBuf)> = Vec::new();
        match self.resolve(path)? {
            Target::Top => {
                for root in &self.roots {
                    self.collect_files(root, Path::new(""), matcher.as_ref(), &mut files);
                }
            }
            Target::In(root, rel) => {
                let shown = root.show(&rel);
                let md = root
                    .dir
                    .metadata(path::for_dir(&rel))
                    .map_err(|e| FolderError::from_io(e, &shown))?;
                if md.is_file() {
                    files.push((root, rel));
                } else {
                    self.collect_files(root, &rel, matcher.as_ref(), &mut files);
                }
            }
        }

        let mut matches = Vec::new();
        let (mut searched, mut skipped_binary, mut skipped_large) = (0usize, 0usize, 0usize);
        let mut truncated = false;
        'files: for (root, file_rel) in files {
            let Ok(fmd) = root.dir.symlink_metadata(&file_rel) else {
                continue;
            };
            if !fmd.is_file() {
                continue;
            }
            if fmd.len() > self.config.max_file_bytes {
                skipped_large += 1;
                continue;
            }
            let Ok(bytes) = root.dir.read(&file_rel) else {
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
                        "path": root.show(&file_rel),
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
        self.require_writable()?;
        let (root, rel) = self.resolve_entry(path)?;
        let shown = root.show(&rel);
        let size = content.len() as u64;
        if size > self.config.max_file_bytes {
            return Err(FolderError::TooLarge {
                path: shown,
                size,
                limit: self.config.max_file_bytes,
                hint: "",
            });
        }
        create_parent(root, &rel, &shown)?;
        let mut opts = OpenOptions::new();
        match mode {
            WriteMode::Create => opts.write(true).create_new(true),
            WriteMode::Overwrite => opts.write(true).create(true).truncate(true),
            WriteMode::Append => opts.append(true).create(true),
        };
        let mut file =
            root.dir
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

    /// `edit`: replace `old_text` with `new_text` in a text file. `old_text` must occur exactly
    /// once unless `replace_all`.
    pub fn edit(
        &self,
        path: &str,
        old_text: &str,
        new_text: &str,
        replace_all: bool,
    ) -> Result<Value, FolderError> {
        self.require_writable()?;
        if old_text.is_empty() {
            return Err(FolderError::Invalid(
                "old_text is empty; give the exact text to replace".into(),
            ));
        }
        let (root, rel) = self.resolve_entry(path)?;
        let shown = root.show(&rel);
        let md = root
            .dir
            .metadata(&rel)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        if md.is_dir() {
            return Err(FolderError::Invalid(format!(
                "{shown} is a directory; edit works on files"
            )));
        }
        let limit = self.config.max_file_bytes;
        if md.len() > limit {
            return Err(FolderError::TooLarge {
                path: shown,
                size: md.len(),
                limit,
                hint: "",
            });
        }
        let bytes = root
            .dir
            .read(&rel)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        if is_binary(&bytes) {
            return Err(FolderError::Invalid(format!(
                "{shown} is a binary file; edit works on text files"
            )));
        }
        let text = String::from_utf8(bytes).map_err(|_| {
            FolderError::Invalid(format!(
                "{shown} is not valid UTF-8; edit it with write instead"
            ))
        })?;
        let count = text.matches(old_text).count();
        if count == 0 {
            return Err(FolderError::Invalid(format!(
                "old_text was not found in {shown}; read the file and copy the text exactly, whitespace included"
            )));
        }
        if count > 1 && !replace_all {
            return Err(FolderError::Invalid(format!(
                "old_text occurs {count} times in {shown}; include more surrounding text so it occurs exactly once, or pass replace_all: true"
            )));
        }
        let updated = if replace_all {
            text.replace(old_text, new_text)
        } else {
            text.replacen(old_text, new_text, 1)
        };
        let size = updated.len() as u64;
        if size > limit {
            return Err(FolderError::TooLarge {
                path: shown,
                size,
                limit,
                hint: " after the edit",
            });
        }
        let mut opts = OpenOptions::new();
        opts.write(true).truncate(true);
        let mut file = root
            .dir
            .open_with(&rel, &opts)
            .map_err(|e| FolderError::from_io(e, &shown))?;
        file.write_all(updated.as_bytes())
            .map_err(|e| FolderError::from_io(e, &shown))?;
        Ok(json!({ "path": shown, "replacements": count, "bytes_written": size }))
    }

    /// `move`: rename `from` to `to` (within a root or across roots); refuses to replace `to`
    /// unless `overwrite`.
    pub fn move_entry(&self, from: &str, to: &str, overwrite: bool) -> Result<Value, FolderError> {
        self.require_writable()?;
        let (from_root, from_rel) = self.resolve_entry(from)?;
        let (to_root, to_rel) = self.resolve_entry(to)?;
        let (from_shown, to_shown) = (from_root.show(&from_rel), to_root.show(&to_rel));
        from_root
            .dir
            .symlink_metadata(&from_rel)
            .map_err(|e| FolderError::from_io(e, &from_shown))?;
        let same_root = std::ptr::eq(from_root, to_root);
        if same_root && from_rel == to_rel {
            return Err(FolderError::Invalid(format!(
                "{from_shown} and {to_shown} are the same path"
            )));
        }
        if same_root && to_rel.starts_with(&from_rel) {
            return Err(FolderError::Invalid(format!(
                "cannot move {from_shown} into itself"
            )));
        }
        if !overwrite && to_root.dir.symlink_metadata(&to_rel).is_ok() {
            return Err(FolderError::Exists(format!(
                "{to_shown} already exists; pass overwrite: true to replace it"
            )));
        }
        create_parent(to_root, &to_rel, &to_shown)?;
        from_root
            .dir
            .rename(&from_rel, &to_root.dir, &to_rel)
            .map_err(|e| {
                if !same_root && e.kind() == std::io::ErrorKind::CrossesDevices {
                    FolderError::Invalid(format!(
                        "{from_shown} and {to_shown} are on different file systems; read and write the file instead"
                    ))
                } else {
                    FolderError::from_io(e, &from_shown)
                }
            })?;
        Ok(json!({ "from": from_shown, "to": to_shown }))
    }

    /// `stat`: kind, size, modified time, and line count for a text file within the size limit.
    pub fn stat(&self, path: &str) -> Result<Value, FolderError> {
        let (root, rel) = match self.resolve(path)? {
            Target::Top => {
                let names: Vec<&str> = self.roots.iter().map(|r| r.name.as_str()).collect();
                return Ok(json!({ "path": ".", "kind": "dir", "roots": names }));
            }
            Target::In(root, rel) => (root, rel),
        };
        let shown = root.show(&rel);
        let md = root
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
            let bytes = root
                .dir
                .read(&rel)
                .map_err(|e| FolderError::from_io(e, &shown))?;
            if let Some(mime) = image_mime(&bytes) {
                out["binary"] = json!(true);
                out["image"] = json!(mime);
            } else if is_binary(&bytes) {
                out["binary"] = json!(true);
            } else {
                out["binary"] = json!(false);
                out["lines"] = json!(count_lines(&bytes));
            }
        }
        Ok(out)
    }

    /// Number of files under all roots the walker sees, stopping once `stop_after` is passed.
    pub fn count_files(&self, stop_after: usize) -> usize {
        let mut n = 0usize;
        for root in &self.roots {
            n += self
                .walker(root, Path::new(""))
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .take(stop_after.saturating_add(1) - n)
                .count();
            if n > stop_after {
                break;
            }
        }
        n
    }

    /* --- helpers ------------------------------------------------------------------------------ */

    /// Resolve a tool path: with one root, a root-relative path; with several, the first
    /// component names the root and `.` is the virtual top.
    fn resolve(&self, input: &str) -> Result<Target<'_>, FolderError> {
        let rel = path::relative(input)?;
        if self.roots.len() == 1 {
            return Ok(Target::In(&self.roots[0], rel));
        }
        let mut parts = rel.components();
        let Some(Component::Normal(first)) = parts.next() else {
            return Ok(Target::Top);
        };
        let first = first.to_string_lossy();
        let root = self.roots.iter().find(|r| r.name == first).ok_or_else(|| {
            let names: Vec<&str> = self.roots.iter().map(|r| r.name.as_str()).collect();
            FolderError::NotFound(format!(
                "{input}: there is no root named {first}; paths start with one of: {}",
                names.join(", ")
            ))
        })?;
        Ok(Target::In(root, parts.as_path().to_path_buf()))
    }

    /// Like [`Self::resolve`], for tools that act on one entry: a root itself is refused.
    fn resolve_entry(&self, input: &str) -> Result<(&Root, PathBuf), FolderError> {
        match self.resolve(input)? {
            Target::In(root, rel) if !rel.as_os_str().is_empty() => Ok((root, rel)),
            Target::In(root, _) if root.prefixed => Err(FolderError::Invalid(format!(
                "{} is a root folder; give a file path inside it",
                root.name
            ))),
            _ => Err(FolderError::Invalid(
                "a file path is required; the root folder itself is not a file".into(),
            )),
        }
    }

    fn require_writable(&self) -> Result<(), FolderError> {
        if self.config.writable {
            Ok(())
        } else {
            Err(FolderError::ReadOnly)
        }
    }

    fn cap(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.config.max_results)
            .clamp(1, self.config.max_results.max(1))
    }

    fn require_dir(&self, root: &Root, rel: &Path, asked: &str) -> Result<(), FolderError> {
        let md = root
            .dir
            .metadata(path::for_dir(rel))
            .map_err(|e| FolderError::from_io(e, asked))?;
        if md.is_dir() {
            Ok(())
        } else {
            Err(FolderError::Invalid(format!(
                "{} is not a directory; use read or stat",
                root.show(rel)
            )))
        }
    }

    /// Append the entries under `rel` to `entries`; true when `limit` cut the listing short.
    fn list_in(
        &self,
        root: &Root,
        rel: &Path,
        matcher: Option<&GlobMatcher>,
        recursive: bool,
        limit: usize,
        entries: &mut Vec<Value>,
    ) -> bool {
        let mut walker = self.walker(root, rel);
        if !recursive {
            walker.max_depth(Some(1));
        }
        for entry in walker.build().flatten() {
            if entry.depth() == 0 {
                continue;
            }
            let Some(entry_rel) = root.to_relative(entry.path()) else {
                continue;
            };
            let shown = root.show(&entry_rel);
            if matcher.is_some_and(|m| !m.is_match(&shown)) {
                continue;
            }
            // Re-open through the capability handle; anything it refuses is not listed.
            let Ok(md) = root.dir.symlink_metadata(&entry_rel) else {
                continue;
            };
            if entries.len() == limit {
                return true;
            }
            let mut item = json!({ "path": shown, "kind": kind(&md) });
            if md.is_file() {
                item["size"] = json!(md.len());
            }
            entries.push(item);
        }
        false
    }

    /// Files under `rel` (a directory) that pass the glob.
    fn collect_files<'a>(
        &self,
        root: &'a Root,
        rel: &Path,
        matcher: Option<&GlobMatcher>,
        files: &mut Vec<(&'a Root, PathBuf)>,
    ) {
        files.extend(
            self.walker(root, rel)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .filter_map(|e| root.to_relative(e.path()))
                .filter(|r| matcher.is_none_or(|m| m.is_match(root.show(r))))
                .map(|r| (root, r)),
        );
    }

    /// A walker over `rel` in `root` (already checked to be a directory through the `Dir`).
    fn walker(&self, root: &Root, rel: &Path) -> WalkBuilder {
        let mut b = WalkBuilder::new(root.path.join(rel));
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
}

fn create_parent(root: &Root, rel: &Path, shown: &str) -> Result<(), FolderError> {
    if let Some(parent) = rel.parent()
        && !parent.as_os_str().is_empty()
    {
        match root.dir.create_dir_all(parent) {
            Ok(()) => {}
            // cap-std answers "exists" for a symlink in the way; resolve it to say whether
            // it is a usable directory or a link out of the root.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match root.dir.metadata(parent) {
                    Ok(md) if md.is_dir() => {}
                    Ok(_) => {
                        return Err(FolderError::Invalid(format!(
                            "{} is not a directory",
                            root.show(parent)
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

/// A root name is one plain path component.
fn valid_root_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && name.trim() == name
}

/// The image type, from the file's magic bytes: PNG, JPEG, GIF or WebP.
fn image_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Characters in UTF-8 bytes (invalid bytes count as one each, as lossy decoding shows them).
fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count()
}

/// Byte offset of the `n`th character (or the end), never inside a UTF-8 sequence.
fn byte_index_of_char(bytes: &[u8], n: usize) -> usize {
    let mut seen = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if (b & 0xC0) != 0x80 {
            if seen == n {
                return i;
            }
            seen += 1;
        }
    }
    bytes.len()
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

    #[test]
    fn root_specs_parse_optional_names() {
        assert_eq!("docs".parse::<RootSpec>().unwrap(), RootSpec::new("docs"));
        assert_eq!(
            "notes=/home/me/notes".parse::<RootSpec>().unwrap(),
            RootSpec::named("notes", "/home/me/notes")
        );
        // A separator before the `=` means it is part of the path.
        assert_eq!("./a=b".parse::<RootSpec>().unwrap(), RootSpec::new("./a=b"));
        assert!("x=".parse::<RootSpec>().is_err());
        assert!("".parse::<RootSpec>().is_err());
        assert!(valid_root_name("docs") && !valid_root_name("..") && !valid_root_name("a/b"));
    }

    #[test]
    fn images_are_sniffed_by_magic_bytes() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(image_mime(b"plain"), None);
    }

    #[test]
    fn character_counting_respects_utf8() {
        let s = "żółw🐢a".as_bytes();
        assert_eq!(count_chars(s), 6);
        assert_eq!(byte_index_of_char(s, 4), "żółw".len());
        assert_eq!(byte_index_of_char(s, 99), s.len());
    }
}
