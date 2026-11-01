# emperor-folder — AI agent notes

**Crate**: `emperor-folder` · **Version**: 0.1.1 · **Licence**: MIT OR Apache-2.0 · **Rust**: 1.88+ (`rust-version` in `Cargo.toml`)

## Scope

A standalone **MCP server confined to one root folder (or several named ones)**, built on
[`emperor-mcp`](https://github.com/yarenty/emperor-mcp) (rmcp underneath). Seven tools: `list`,
`read`, `search`, `stat`, and with `--writable` also `write`, `edit`, `move`. Streamable HTTP via
`emperor_mcp::serve` (stateless, `/mcp` + `/health`), stdio via rmcp for desktop clients.
Read-only unless started with `--writable`.

Layout (one package, library + binary):

- `src/path.rs` — tool path → root-relative path; refuses absolute paths and any `..`.
- `src/folder.rs` — `Folder`: each root as a `cap_std::fs::Dir`, tool-path resolution across
  roots, the seven operations (synchronous), limits, binary and image sniffing, the `ignore`
  walker.
- `src/server.rs` — `FolderServer`: the rmcp `#[tool_router]` with annotations, write tools
  removed from the router when read-only, argument structs, framing, `serve_http` /
  `serve_stdio`, default `Host` validation.
- `src/doctor.rs` — the `doctor` report on `emperor_mcp::doctor`.
- `src/error.rs` — `FolderError`, messages written for the model.
- `src/main.rs` — clap CLI (`serve`, `doctor`), tracing to stderr.
- `tests/tools.rs` — every tool over real HTTP on a temp folder, tool-list conformance
  (`emperor_mcp::testkit`), escape attempts (also across roots), read-only mode, ignore rules,
  limits, images, framing breakout, `Host` and `Origin` validation.
- `tests/cli.rs` — the built binary: `doctor` exit codes, a `serve --stdio` round trip.

## Before you change code

1. Read `src/folder.rs` (module docs: how confinement works) and `src/server.rs`.
2. Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
   and `cargo deny check` (licences, advisories, sources; config in `deny.toml`).

## Conventions

- **Every file operation goes through a `Dir`.** `Folder` holds each root as a
  `cap_std::fs::Dir`; open, read, write, metadata, create_dir_all and rename are calls on it with
  a root-relative path. Never `std::fs` with a joined absolute path. The only absolute path in
  use is the walker's start directory (`ignore` cannot walk a `Dir`), and every path it yields is
  made relative and re-opened through the `Dir`.
- **Paths are checked twice, on purpose.** `path::relative` refuses absolute paths and `..` with
  a clear message; cap-std refuses whatever still resolves outside (symlinks). Do not relax the
  first check because the second exists, and do not rely on the first alone.
- **Escape tests are the contract.** A change to path handling, a new tool argument that takes a
  path, or a new operation gets a case in `escapes_are_refused_by_every_tool` and, for anything
  that follows links, in `symlinks_pointing_outside_are_refused`.
- **Several roots.** One root: tool paths are plain root-relative paths (keep that identical).
  Several: the first component is the root's name, `.` is the virtual top. Every operation
  resolves through `Folder::resolve` / `resolve_entry`; paths shown to the model come from
  `Root::show`.
- **Seven tools.** emperor-mcp's budget is 3 to 8 (`doctor` checks it). A new capability is an
  argument on an existing tool or a separate server, not an eighth tool without a reason.
- **Annotations on every tool.** `read_only_hint`, `destructive_hint`, `idempotent_hint`,
  `open_world_hint = false`; `tests/tools.rs` checks them with `emperor_mcp::testkit`.
- **Read-only by default.** `write`, `edit` and `move` (`server::WRITE_TOOLS`) are removed from
  the router when not `--writable`, so `tools/list` does not offer them; the `Folder`
  operations also refuse with `FolderError::ReadOnly`. A new mutating tool joins `WRITE_TOOLS`.
- **Framing.** File content: `frame(FrameKind::Document, …)` (its author is not known). Images:
  MCP image content plus a framed `Computed` note. Structured results: JSON
  text framed `FrameKind::Computed`. Errors are tool errors (`isError: true`), unframed, written
  for the model (what was wrong, what to do).
- **Limits live in `Config`.** `max_file_bytes`, `max_results` and `max_chars` bound every read,
  search, edit, write and listing; a new code path that reads file content checks them.
  `max_chars` comes from `emperor_mcp::limits` (default and ceiling); `max_results` and
  `max_file_bytes` keep their own defaults because the emperor-mcp ceilings (100 results,
  64 MiB) would change existing flags.
- **Blocking work on the blocking pool.** Tool handlers call `FolderServer::run`
  (`spawn_blocking`); `Folder` stays synchronous and free of tokio.
- **Dependencies.** Community-maintained crates only: `cap-std` (Bytecode Alliance), `ignore`
  and `globset` (ripgrep's walker and its glob matcher, already a dependency of `ignore`),
  `regex` (rust-lang), `base64` (image content, already in the tree through rmcp), `clap`,
`tokio`, `serde`, `thiserror`/`anyhow`, `tracing`. A new
  dependency needs a reason written here. `tracing-subscriber` is used directly (not
  `emperor_mcp::init_tracing`) because logs must go to stderr in stdio mode.
- **emperor-mcp from crates.io** (`0.3`; `testkit` feature in dev-dependencies);
  `deny.toml` allows no git sources. The CLI keeps its own transport flags rather than
  flattening `emperor_mcp::cli::ServeArgs`, whose `--stdio` conflicts with `--bind` and
  `--allowed-host`, which this CLI has always accepted together.
- **CLI flags are a public contract.** Changing or removing one is a breaking change.
- Licence is dual `MIT OR Apache-2.0`; keep `Cargo.toml`, the README badge and the README
  licence section in agreement. Contributions are signed off (DCO, see `CONTRIBUTING.md`).

## Documentation closure (mandatory)

After any behaviour change, update **[`README.md`](./README.md)** (flags, tools table, security
model), **[`manifest.yaml`](./manifest.yaml)** when a tool or a flag changes, and add a
[`CHANGELOG.md`](./CHANGELOG.md) entry under `[Unreleased]` for anything a user would notice.
**Shipping code without updating docs is incomplete work.**

## Related docs

- [`README.md`](./README.md) — install, run, connect, tools, security model, doctor
- [`CHANGELOG.md`](./CHANGELOG.md) — what each release changed
- [`manifest.yaml`](./manifest.yaml) — tools and settings for host applications
- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — checks, DCO
- [`server.json`](./server.json) — official MCP Registry entry; its two `version` fields follow
  `Cargo.toml` on every release
- [`Dockerfile`](./Dockerfile) — stdio image for container catalogs, folder mounted at `/data`
