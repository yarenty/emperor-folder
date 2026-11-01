//! `emperor-folder` CLI: `serve` and `doctor`.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use emperor_folder::{
    Config, Folder, RootSpec, ServeOptions, default_allowed_hosts, doctor, serve_http, serve_stdio,
};

#[derive(Parser)]
#[command(
    name = "emperor-folder",
    version,
    about = "MCP server confined to one folder"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server (Streamable HTTP at /mcp, or stdio with --stdio).
    Serve(Opts),
    /// Check the root folder, the listen address and the tool budget; exit 1 on failure.
    Doctor(Opts),
}

#[derive(Args)]
struct Opts {
    /// A folder the server is confined to. Repeat for several roots; NAME=DIR names one
    /// (default: the folder's name). With several roots, paths start with the root's name.
    #[arg(long = "root", value_name = "[NAME=]DIR", required = true)]
    roots: Vec<RootSpec>,
    /// Address for Streamable HTTP.
    #[arg(long, default_value = "127.0.0.1:8031")]
    bind: SocketAddr,
    /// Offer the write, edit and move tools (default: read-only, they are not listed).
    #[arg(long)]
    writable: bool,
    /// Speak MCP over stdin/stdout instead of HTTP (desktop clients).
    #[arg(long)]
    stdio: bool,
    /// Include hidden files and directories in list and search.
    #[arg(long)]
    hidden: bool,
    /// Do not honour .gitignore / .ignore files in list and search.
    #[arg(long)]
    no_ignore: bool,
    /// Largest file read, searched or written, in bytes.
    #[arg(long, default_value_t = emperor_folder::folder::DEFAULT_MAX_FILE_BYTES)]
    max_file_bytes: u64,
    /// Most entries list returns and most matches search returns.
    #[arg(long, default_value_t = emperor_folder::folder::DEFAULT_MAX_RESULTS)]
    max_results: usize,
    /// Most characters of text one read returns (at most 200000).
    #[arg(long, default_value_t = emperor_folder::folder::DEFAULT_MAX_CHARS)]
    max_chars: usize,
    /// Accepted Host header (repeatable). Default: loopback names on a loopback bind,
    /// no check otherwise (put a validating proxy in front).
    #[arg(long = "allowed-host", value_name = "HOST")]
    allowed_hosts: Vec<String>,
    /// Accepted Origin header, with scheme, e.g. https://app.example.com (repeatable).
    /// Requests without an Origin header (non-browser clients) still pass.
    #[arg(long = "allowed-origin", value_name = "ORIGIN")]
    allowed_origins: Vec<String>,
}

impl Opts {
    fn config(&self) -> Config {
        let mut config = Config::with_roots(self.roots.clone()).max_chars(self.max_chars);
        config.writable = self.writable;
        config.hidden = self.hidden;
        config.no_ignore = self.no_ignore;
        config.max_file_bytes = self.max_file_bytes;
        config.max_results = self.max_results.max(1);
        config
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor(opts) => {
            let report = doctor::report(&opts.config(), (!opts.stdio).then_some(opts.bind));
            report.print();
            std::process::exit(report.exit_code());
        }
        Command::Serve(opts) => {
            init_tracing();
            let folder = Arc::new(Folder::open(opts.config())?);
            for (name, path) in folder.roots() {
                tracing::info!(root = %path.display(), name, "root folder");
            }
            tracing::info!(writable = opts.writable, "emperor-folder starting");
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async move {
                if opts.stdio {
                    serve_stdio(folder).await
                } else {
                    let hosts = if opts.allowed_hosts.is_empty() {
                        default_allowed_hosts(opts.bind)
                    } else {
                        opts.allowed_hosts.clone()
                    };
                    let options = ServeOptions::new(opts.bind)
                        .with_allowed_hosts(hosts)
                        .with_allowed_origins(opts.allowed_origins.clone());
                    serve_http(folder, options).await
                }
            })
        }
    }
}

/// Logs to stderr in both modes: in stdio mode stdout is the protocol stream.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .try_init();
}
