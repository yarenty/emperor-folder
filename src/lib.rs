//! An MCP server confined to one folder.
//!
//! The operator names a root directory; the model gets six tools (`list`, `read`, `search`,
//! `write`, `move`, `stat`) over it and nothing else. Confinement is enforced by
//! [`cap_std::fs::Dir`]: every file operation is a root-relative call on the opened root, and the
//! operating system refuses anything that resolves outside it, symlinks included. Listing and
//! search use the [`ignore`] walker, so `.gitignore` and hidden files are honoured. The server is
//! read-only unless started with `--writable`.
//!
//! Built on [emperor-mcp](https://github.com/yarenty/emperor-mcp): stateless Streamable HTTP,
//! framed output, a `doctor` subcommand. stdio is available for desktop clients.

#![warn(missing_docs)]

pub mod doctor;
pub mod error;
pub mod folder;
pub mod path;
pub mod server;

pub use error::FolderError;
pub use folder::{Config, Folder, ReadOutput, WriteMode};
pub use server::{FolderServer, default_allowed_hosts, serve_http, serve_stdio};
