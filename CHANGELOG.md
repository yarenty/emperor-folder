# Changelog

All notable changes to emperor-folder. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-01

> One folder, nothing else.

### Added
- MCP server confined to one root folder (`--root`), on [emperor-mcp](https://github.com/yarenty/emperor-mcp) 0.2: stateless Streamable HTTP at `/mcp` with `/health`, or stdio (`--stdio`) for desktop clients.
- Six tools: `list`, `read` (whole file or a line range), `search` (regex), `write` (create, overwrite, append), `move`, `stat`.
- Confinement through a cap-std directory handle: absolute paths, `..` and symlinks that leave the root are refused.
- `.gitignore`, `.ignore` and hidden files honoured by `list` and `search` (`--no-ignore`, `--hidden` to widen); symlinks never followed by the walker.
- Read-only by default; `--writable` enables `write` and `move`.
- Limits: `--max-file-bytes` (default 1 MiB), `--max-results` (default 200); binary files reported, not returned.
- Framed output: file content as reference data, listings and matches as computed JSON.
- `Host` header validation on loopback binds (`--allowed-host` to name others).
- `doctor` subcommand: root folder, read and write access, listen address, tool budget, file count.
- Release binaries for four targets.

[Unreleased]: https://github.com/yarenty/emperor-folder/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yarenty/emperor-folder/releases/tag/v0.1.0
