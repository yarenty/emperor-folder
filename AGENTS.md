# emperor-folder — AI agent notes

**Crate**: `emperor-folder` · **Version**: 0.1.0 · **Licence**: MIT OR Apache-2.0 · **Rust**: 1.88+ (`rust-version` in `Cargo.toml`)

## Scope

A standalone **MCP server confined to one root folder**, built on
[`emperor-mcp`](https://github.com/yarenty/emperor-mcp) (rmcp underneath). Six tools: `list`,
`read`, `search`, `write`, `move`, `stat`. Streamable HTTP via `emperor_mcp::serve` (stateless,
`/mcp` + `/health`), stdio via rmcp for desktop clients. Read-only unless started with
`--writable`.

Layout (one package, library + binary):

- `src/path.rs` — tool path → root-relative path; refuses absolute paths and any `..`.
- `src/folder.rs` — `Folder`: the root as a `cap_std::fs::Dir` plus the six operations
  (synchronous), limits, binary sniffing, the `ignore` walker.
- `src/server.rs` — `FolderServer`: the rmcp `#[tool_router]`, argument structs, framing,
  `serve_http` / `serve_stdio`, default `Host` validation.
- `src/doctor.rs` — the `doctor` report on `emperor_mcp::doctor`.
- `src/error.rs` — `FolderError`, messages written for the model.
- `src/main.rs` — clap CLI (`serve`, `doctor`), tracing to stderr.
- `tests/tools.rs` — every tool over real HTTP on a temp folder, escape attempts, read-only mode,
  ignore rules, limits, framing breakout, `Host` validation.
- `tests/cli.rs` — the built binary: `doctor` exit codes, a `serve --stdio` round trip.

## Before you change code

1. Read `src/folder.rs` (module docs: how confinement works) and `src/server.rs`.
2. Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
   and `cargo deny check` (licences, advisories, sources; config in `deny.toml`).

## Conventions

- **Every file operation goes through the `Dir`.** `Folder` holds the root as a
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
- **Six tools.** emperor-mcp's budget is 3 to 8 (`doctor` checks it). A new capability is an
  argument on an existing tool or a separate server, not a seventh tool without a reason.
- **Read-only by default.** `write` and `move` stay listed when read-only and answer
  `FolderError::ReadOnly`; new mutating operations follow the same pattern.
- **Framing.** File content: `frame(FrameKind::TrustedReference, …)`. Structured results: JSON
  text framed `FrameKind::Computed`. Errors are tool errors (`isError: true`), unframed, written
  for the model (what was wrong, what to do).
- **Limits live in `Config`.** `max_file_bytes` and `max_results` bound every read, search,
  write and listing; a new code path that reads file content checks them.
- **Blocking work on the blocking pool.** Tool handlers call `FolderServer::run`
  (`spawn_blocking`); `Folder` stays synchronous and free of tokio.
- **Dependencies.** Community-maintained crates only: `cap-std` (Bytecode Alliance), `ignore`
  and `globset` (ripgrep's walker and its glob matcher, already a dependency of `ignore`),
  `regex` (rust-lang), `clap`, `tokio`, `serde`, `thiserror`/`anyhow`, `tracing`. A new
  dependency needs a reason written here. `tracing-subscriber` is used directly (not
  `emperor_mcp::init_tracing`) because logs must go to stderr in stdio mode.
- **emperor-mcp from crates.io** (`0.2`); `deny.toml` allows no git sources.
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
