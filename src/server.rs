//! The MCP surface: seven tools over one [`Folder`], served over Streamable HTTP (emperor-mcp)
//! or stdio (rmcp).
//!
//! File content is framed [`FrameKind::Document`] (a file's author is not known); structured
//! results (listings, matches, metadata) are JSON framed [`FrameKind::Computed`]; small images
//! come back as MCP image content. Refusals come back as tool errors (`isError: true`) with a
//! message the model can act on.
//!
//! On a read-only server the mutating tools (`write`, `edit`, `move`) are removed from the tool
//! router at construction, so `tools/list` does not offer them at all.

use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine;
use emperor_mcp::schema::CompactSchemas;
use emperor_mcp::{FrameKind, ServeOptions, frame};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::Value;

use crate::error::FolderError;
use crate::folder::{Folder, ReadOutput, WriteMode};

/// Server name reported to clients and by `doctor`.
pub const SERVER_NAME: &str = "emperor-folder";

/// Tools that change files; offered only when the server is writable.
pub const WRITE_TOOLS: [&str; 3] = ["write", "edit", "move"];

/// Arguments of `list`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Directory to list; omit or "." for the top.
    #[serde(default)]
    pub path: Option<String>,
    /// Glob on paths as shown, e.g. "*.md" or "src/**/*.rs".
    #[serde(default)]
    pub glob: Option<String>,
    /// Include subdirectories (default false: one level).
    #[serde(default)]
    pub recursive: bool,
    /// Most entries to return; capped by the server limit.
    #[serde(default)]
    pub max_entries: Option<usize>,
}

/// Arguments of `read`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// File to read.
    pub path: String,
    /// First line to return, 1-based (default 1).
    #[serde(default)]
    pub start_line: Option<usize>,
    /// Last line to return, inclusive (default: end of file).
    #[serde(default)]
    pub end_line: Option<usize>,
}

/// Arguments of `search`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Regular expression (Rust regex syntax) matched against each line.
    pub pattern: String,
    /// Directory or file to search; omit for everything.
    #[serde(default)]
    pub path: Option<String>,
    /// Glob on paths as shown, e.g. "*.rs".
    #[serde(default)]
    pub glob: Option<String>,
    /// Match case-insensitively (default false).
    #[serde(default)]
    pub ignore_case: bool,
}

/// Arguments of `write`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteArgs {
    /// File to write; parent directories are created.
    pub path: String,
    /// Text to write.
    pub content: String,
    /// "create" (default, refuses an existing file), "overwrite" or "append".
    #[serde(default)]
    pub mode: WriteMode,
}

/// Arguments of `edit`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditArgs {
    /// Text file to change.
    pub path: String,
    /// Exact text to replace, whitespace included; must occur exactly once unless replace_all.
    pub old_text: String,
    /// Replacement text (may be empty to delete old_text).
    pub new_text: String,
    /// Replace every occurrence (default false).
    #[serde(default)]
    pub replace_all: bool,
}

/// Arguments of `move`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    /// Existing file or directory.
    pub from: String,
    /// New path; parent directories are created.
    pub to: String,
    /// Replace an existing target (default false).
    #[serde(default)]
    pub overwrite: bool,
}

/// Arguments of `stat`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StatArgs {
    /// Path to describe; "." for the top.
    pub path: String,
}

/// The MCP server: one [`Folder`] shared by every request.
#[derive(Clone)]
pub struct FolderServer {
    folder: Arc<Folder>,
    tool_router: ToolRouter<Self>,
    instructions: String,
}

#[tool_router]
impl FolderServer {
    /// A server over `folder`. Without `--writable`, `write`, `edit` and `move` are not
    /// registered.
    pub fn new(folder: Arc<Folder>) -> Self {
        let writable = folder.config().writable;
        let instructions = instructions(&folder);
        Self {
            folder,
            tool_router: Self::router(writable),
            instructions,
        }
    }

    /// The tool router for a writable or read-only server, with compact input schemas.
    pub fn router(writable: bool) -> ToolRouter<Self> {
        let mut router = Self::tool_router().compact_schemas();
        if !writable {
            for name in WRITE_TOOLS {
                router.remove_route(name);
            }
        }
        router
    }

    /// Number of tools the server exposes (for `doctor`'s tool budget).
    pub fn tool_count(writable: bool) -> usize {
        Self::router(writable).list_all().len()
    }

    #[tool(
        name = "list",
        description = "List files and directories in a folder. Skips hidden and .gitignored entries. Optional glob filter; recursive for subfolders.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        let path = a.path.unwrap_or_default();
        computed(
            self.run(move |f| f.list(&path, a.glob.as_deref(), a.recursive, a.max_entries))
                .await,
        )
    }

    #[tool(
        name = "read",
        description = "Read a text file, optionally only lines start_line..end_line (1-based, inclusive). Small PNG, JPEG, GIF and WebP images come back as images; other binary files are reported, not returned.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn read(&self, Parameters(a): Parameters<ReadArgs>) -> CallToolResult {
        match self
            .run(move |f| f.read(&a.path, a.start_line, a.end_line))
            .await
        {
            Ok(ReadOutput::Text {
                path,
                content,
                first_line,
                last_line,
                total_lines,
                truncated,
                partial_last_line,
            }) => {
                let total = total_lines.map_or_else(|| "?".to_string(), |n| n.to_string());
                let mut header = format!("{path} lines {first_line}-{last_line} of {total}");
                if truncated {
                    let next = if partial_last_line {
                        last_line
                    } else {
                        last_line + 1
                    };
                    header.push_str(&format!(
                        " (cut at the size limit; continue with start_line {next})"
                    ));
                }
                text(frame(FrameKind::Document, &format!("{header}\n{content}")))
            }
            Ok(ReadOutput::Image { path, mime, data }) => {
                let about = serde_json::json!({ "path": path, "mime": mime, "size": data.len() });
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                CallToolResult::success(vec![
                    ContentBlock::text(frame(FrameKind::Computed, &about.to_string())),
                    ContentBlock::image(b64, mime),
                ])
            }
            Ok(ReadOutput::Binary { path, size }) => computed(Ok(serde_json::json!({
                "path": path,
                "binary": true,
                "size": size,
                "note": "binary file, content not returned",
            }))),
            Err(e) => error(e),
        }
    }

    #[tool(
        name = "search",
        description = "Search text files for a regular expression; returns path, line number and line for each match. Optional start path, glob filter, ignore_case.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search(&self, Parameters(a): Parameters<SearchArgs>) -> CallToolResult {
        let path = a.path.unwrap_or_default();
        computed(
            self.run(move |f| f.search(&a.pattern, &path, a.glob.as_deref(), a.ignore_case))
                .await,
        )
    }

    #[tool(
        name = "write",
        description = "Write text to a file. mode: create (default, fails if it exists), overwrite, or append. Creates parent directories.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn write(&self, Parameters(a): Parameters<WriteArgs>) -> CallToolResult {
        computed(
            self.run(move |f| f.write(&a.path, &a.content, a.mode))
                .await,
        )
    }

    #[tool(
        name = "edit",
        description = "Replace exact text in a text file: old_text must occur exactly once (copy it from read, whitespace included) unless replace_all is true.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn edit(&self, Parameters(a): Parameters<EditArgs>) -> CallToolResult {
        computed(
            self.run(move |f| f.edit(&a.path, &a.old_text, &a.new_text, a.replace_all))
                .await,
        )
    }

    #[tool(
        name = "move",
        description = "Move or rename a file or directory. Refuses to replace an existing target unless overwrite is true.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn move_entry(&self, Parameters(a): Parameters<MoveArgs>) -> CallToolResult {
        computed(
            self.run(move |f| f.move_entry(&a.from, &a.to, a.overwrite))
                .await,
        )
    }

    #[tool(
        name = "stat",
        description = "Describe a path: kind (file, dir, symlink), size, modified time, line count for text files, image type for images.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn stat(&self, Parameters(a): Parameters<StatArgs>) -> CallToolResult {
        computed(self.run(move |f| f.stat(&a.path)).await)
    }
}

impl FolderServer {
    /// Run a filesystem operation on the blocking pool.
    async fn run<T, F>(&self, op: F) -> Result<T, FolderError>
    where
        T: Send + 'static,
        F: FnOnce(&Folder) -> Result<T, FolderError> + Send + 'static,
    {
        let folder = Arc::clone(&self.folder);
        tokio::task::spawn_blocking(move || op(&folder))
            .await
            .unwrap_or_else(|e| Err(FolderError::Invalid(format!("internal error: {e}"))))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FolderServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions.clone())
    }
}

/// The instructions sent at `initialize`: how paths work and whether files can be changed.
fn instructions(folder: &Folder) -> String {
    let names: Vec<&str> = folder.roots().map(|(name, _)| name).collect();
    let paths = if names.len() == 1 {
        "Files in one folder. Every path is relative to that folder".to_string()
    } else {
        format!(
            "Files in {} folders: {}. Every path starts with one of those names (e.g. {}/notes.md); \
list \".\" shows them",
            names.len(),
            names.join(", "),
            names[0]
        )
    };
    let access = if folder.config().writable {
        "write creates or replaces a file, edit replaces exact text in one, move renames."
    } else {
        "The server is read-only: no tool changes files."
    };
    format!(
        "{paths}; absolute paths and `..` are refused. list and search skip hidden and \
.gitignored files. {access}"
    )
}

fn text(body: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(body)])
}

fn error(e: FolderError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

fn computed(result: Result<Value, FolderError>) -> CallToolResult {
    match result {
        Ok(v) => {
            let json = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            text(frame(FrameKind::Computed, &json))
        }
        Err(e) => error(e),
    }
}

/// Hosts accepted in the `Host` header when none are configured: loopback names when the
/// server listens on a loopback address (DNS-rebinding protection for a local file server),
/// none (validation off, a validating ingress in front) otherwise.
pub fn default_allowed_hosts(bind: SocketAddr) -> Vec<String> {
    if bind.ip().is_loopback() {
        ["localhost", "127.0.0.1", "::1"]
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    }
}

/// Serve over stateless Streamable HTTP at `/mcp` (plus `/health`) until Ctrl-C.
pub async fn serve_http(folder: Arc<Folder>, options: ServeOptions) -> anyhow::Result<()> {
    emperor_mcp::serve(options, move || FolderServer::new(Arc::clone(&folder))).await
}

/// Serve over stdio (desktop clients). Logs must go to stderr.
pub async fn serve_stdio(folder: Arc<Folder>) -> anyhow::Result<()> {
    let service = FolderServer::new(folder)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_tools_writable_four_read_only() {
        assert_eq!(FolderServer::tool_count(true), 7);
        assert_eq!(FolderServer::tool_count(false), 4);
        let ro = FolderServer::router(false);
        assert!(WRITE_TOOLS.iter().all(|t| !ro.has_route(t)));
    }

    #[test]
    fn tools_conform_in_both_modes() {
        use emperor_mcp::testkit;
        let writable = FolderServer::router(true).list_all();
        let read_only = FolderServer::router(false).list_all();
        for tools in [&writable, &read_only] {
            testkit::assert_tools(tools);
            testkit::assert_compact_schemas(tools);
            assert!(
                testkit::warnings(tools).is_empty(),
                "{:?}",
                testkit::warnings(tools)
            );
        }
        testkit::assert_read_only_default(&read_only);
    }

    #[test]
    fn loopback_bind_validates_host() {
        assert!(!default_allowed_hosts("127.0.0.1:8031".parse().unwrap()).is_empty());
        assert!(default_allowed_hosts("0.0.0.0:8031".parse().unwrap()).is_empty());
    }
}
