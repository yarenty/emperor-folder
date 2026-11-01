//! An MCP server confined to one folder (or a few named ones).
//!
//! The operator names one or more root directories; the model gets seven tools (`list`, `read`,
//! `search`, `stat`, and with `--writable` also `write`, `edit`, `move`) over them and nothing
//! else. Confinement is enforced by
//! [`cap_std::fs::Dir`]: every file operation is a root-relative call on the opened root, and the
//! operating system refuses anything that resolves outside its root, symlinks included. Listing and
//! search use the [`ignore`] walker, so `.gitignore` and hidden files are honoured. The server is
//! read-only unless started with `--writable`; read-only, the mutating tools are not listed.
//!
//! Built on [emperor-mcp](https://github.com/yarenty/emperor-mcp): stateless Streamable HTTP,
//! framed output, a `doctor` subcommand. stdio is available for desktop clients.

#![warn(missing_docs)]

pub mod doctor;
pub mod error;
pub mod folder;
pub mod path;
pub mod server;

pub use emperor_mcp::ServeOptions;
pub use error::FolderError;
pub use folder::{Config, Folder, ReadOutput, RootSpec, WriteMode};
pub use server::{FolderServer, default_allowed_hosts, serve_http, serve_stdio};
