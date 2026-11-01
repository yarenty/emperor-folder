# Changelog

All notable changes to emperor-folder. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-01

> One folder, nothing else.

### Added
- MCP server confined to one root folder (`--root`), on [emperor-mcp](https://github.com/yarenty/emperor-mcp) 0.3: stateless Streamable HTTP at `/mcp` with `/health`, or stdio (`--stdio`) for desktop clients.
- Several roots: `--root` is repeatable and takes `NAME=DIR` (default name: the folder's own). With several roots, paths start with the root's name and `.` lists the roots; with one root, paths are plain relative paths.
- Seven tools: `list`, `read` (whole file or a line range), `search` (regex), `stat`, and with `--writable` `write` (create, overwrite, append), `edit` (replace one exact text snippet, or every occurrence with `replace_all`) and `move` (also between roots).
- Tool annotations on every tool: `list`, `read`, `search` and `stat` read-only and idempotent; `write`, `edit` and `move` destructive; none open-world.
- `read` returns PNG, JPEG, GIF and WebP images up to 5 MiB as MCP image content; `stat` names the image type.
- Confinement through one cap-std directory handle per root: absolute paths, `..` and symlinks that leave a root (also into another root) are refused.
- `.gitignore`, `.ignore` and hidden files honoured by `list` and `search` (`--no-ignore`, `--hidden` to widen); symlinks never followed by the walker.
- Read-only by default: without `--writable`, `write`, `edit` and `move` are not listed at all.
- Limits: `--max-file-bytes` (default 1 MiB), `--max-results` (default 200), `--max-chars` (default 50 000 characters per `read`, cut at a line boundary with a note on where to continue); binary files other than small images reported, not returned.
- Framed output: file content as a document of unknown authorship, listings and matches as computed JSON.
- `Host` header validation on loopback binds (`--allowed-host` to name others); `Origin` validation with `--allowed-origin`.
- `doctor` subcommand: each root readable (and writable with `--writable`), root names, listen address, tool budget, file count.
- Release binaries for four targets, with a `SHA256SUMS` file and build provenance attestations.
- Issue templates (bug, feature) and a pull request template.

[Unreleased]: https://github.com/yarenty/emperor-folder/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yarenty/emperor-folder/releases/tag/v0.1.0
