//! The MCP surface: six tools over one [`Folder`], served over Streamable HTTP (emperor-mcp) or
//! stdio (rmcp).
//!
//! File content is framed [`FrameKind::TrustedReference`]; structured results (listings,
//! matches, metadata) are JSON framed [`FrameKind::Computed`]. Refusals come back as tool errors
//! (`isError: true`) with a message the model can act on.

use std::net::SocketAddr;
use std::sync::Arc;

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

const INSTRUCTIONS: &str = "Files in one folder. Every path is relative to that folder; \
absolute paths and `..` are refused. list and search skip hidden and .gitignored files. \
write and move work only when the operator started the server with --writable.";

/// Arguments of `list`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Directory relative to the root folder; omit or "." for the root.
    #[serde(default)]
    pub path: Option<String>,
    /// Glob on root-relative paths, e.g. "*.md" or "src/**/*.rs".
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
    /// File relative to the root folder.
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
    /// Directory or file to search, relative to the root; omit for the whole folder.
    #[serde(default)]
    pub path: Option<String>,
    /// Glob on root-relative paths, e.g. "*.rs".
    #[serde(default)]
    pub glob: Option<String>,
    /// Match case-insensitively (default false).
    #[serde(default)]
    pub ignore_case: bool,
}

/// Arguments of `write`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteArgs {
    /// File relative to the root folder; parent directories are created.
    pub path: String,
    /// Text to write.
    pub content: String,
    /// "create" (default, refuses an existing file), "overwrite" or "append".
    #[serde(default)]
    pub mode: WriteMode,
}

/// Arguments of `move`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    /// Existing file or directory, relative to the root.
    pub from: String,
    /// New path, relative to the root; parent directories are created.
    pub to: String,
    /// Replace an existing target (default false).
    #[serde(default)]
    pub overwrite: bool,
}

/// Arguments of `stat`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StatArgs {
    /// Path relative to the root folder; "." for the root.
    pub path: String,
}

/// The MCP server: one [`Folder`] shared by every request.
#[derive(Clone)]
pub struct FolderServer {
    folder: Arc<Folder>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl FolderServer {
    /// A server over `folder`.
    pub fn new(folder: Arc<Folder>) -> Self {
        Self {
            folder,
            tool_router: Self::tool_router(),
        }
    }

    /// Number of tools the server exposes (for `doctor`'s tool budget).
    pub fn tool_count() -> usize {
        Self::tool_router().list_all().len()
    }

    #[tool(
        name = "list",
        description = "List files and directories in a folder (root-relative). Skips hidden and .gitignored entries. Optional glob filter; recursive for subfolders."
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
        description = "Read a text file (root-relative path), optionally only lines start_line..end_line (1-based, inclusive). Binary files are reported, not returned."
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
            }) => {
                let total = total_lines.map_or_else(|| "?".to_string(), |n| n.to_string());
                let mut header = format!("{path} lines {first_line}-{last_line} of {total}");
                if truncated {
                    header.push_str(" (cut at the size limit)");
                }
                text(frame(
                    FrameKind::TrustedReference,
                    &format!("{header}\n{content}"),
                ))
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
        description = "Search text files for a regular expression; returns path, line number and line for each match. Optional start path, glob filter, ignore_case."
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
        description = "Write text to a file (root-relative path). mode: create (default, fails if it exists), overwrite, or append. Creates parent directories."
    )]
    async fn write(&self, Parameters(a): Parameters<WriteArgs>) -> CallToolResult {
        computed(
            self.run(move |f| f.write(&a.path, &a.content, a.mode))
                .await,
        )
    }

    #[tool(
        name = "move",
        description = "Move or rename a file or directory inside the root folder. Refuses to replace an existing target unless overwrite is true."
    )]
    async fn move_entry(&self, Parameters(a): Parameters<MoveArgs>) -> CallToolResult {
        computed(
            self.run(move |f| f.move_entry(&a.from, &a.to, a.overwrite))
                .await,
        )
    }

    #[tool(
        name = "stat",
        description = "Describe a path: kind (file, dir, symlink), size, modified time, and line count for text files."
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
            .with_instructions(INSTRUCTIONS)
    }
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
pub async fn serve_http(
    folder: Arc<Folder>,
    bind: SocketAddr,
    allowed_hosts: Vec<String>,
) -> anyhow::Result<()> {
    let options = ServeOptions::new(bind).with_allowed_hosts(allowed_hosts);
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
    fn exactly_six_tools() {
        assert_eq!(FolderServer::tool_count(), 6);
    }

    #[test]
    fn loopback_bind_validates_host() {
        assert!(!default_allowed_hosts("127.0.0.1:8031".parse().unwrap()).is_empty());
        assert!(default_allowed_hosts("0.0.0.0:8031".parse().unwrap()).is_empty());
    }
}
