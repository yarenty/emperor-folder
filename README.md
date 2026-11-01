# emperor-folder

[![CI](https://github.com/yarenty/emperor-folder/actions/workflows/ci.yml/badge.svg)](https://github.com/yarenty/emperor-folder/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**An MCP server confined to one folder.** You name a directory; the model can list, read,
search, and (if you allow it) write and move files inside it, and cannot reach anything outside
it: not with an absolute path, not with `..`, not through a symlink.

```bash
emperor-folder serve --root ~/notes            # read-only, Streamable HTTP on 127.0.0.1:8031/mcp
emperor-folder serve --root ~/notes --writable # also write and move
emperor-folder serve --root ~/notes --stdio    # for desktop clients
```

## Why

Giving an agent "the filesystem" is the fastest way to give it your SSH keys. Most file tools
check paths with string prefixes and then open them with the full authority of the process; a
symlink, a `..` the check missed, or a race between check and open, and the model is out.

emperor-folder does not check-then-open. It opens the root once as a
[`cap-std`](https://github.com/bytecodealliance/cap-std) directory handle and does every file
operation *through that handle* with a relative path, so resolution happens under the
operating system's (or cap-std's) beneath-the-root rules. There is no code path that opens a
file by absolute path.

It is built on [emperor-mcp](https://github.com/yarenty/emperor-mcp): stateless Streamable HTTP
(no session id, load-balancer friendly), output framed so file content reaches the model as
data rather than instructions, and a `doctor` subcommand.

## Install

Release binaries for Linux (x86_64), macOS (Apple Silicon and Intel) and Windows are attached
to each [GitHub release](https://github.com/yarenty/emperor-folder/releases). From source
(Rust 1.88 or newer):

```bash
cargo install --git https://github.com/yarenty/emperor-folder
```

## Run

```bash
emperor-folder doctor --root ~/notes   # check first: exits 1 if something is wrong
emperor-folder serve  --root ~/notes
```

| Flag | Default | Meaning |
|---|---|---|
| `--root DIR` | (required) | The only folder the server can see. |
| `--bind ADDR` | `127.0.0.1:8031` | Streamable HTTP address; MCP endpoint at `/mcp`, liveness at `/health`. |
| `--writable` | off | Enable `write` and `move`. Without it they stay listed and answer an error. |
| `--stdio` | off | Speak MCP over stdin/stdout instead of HTTP. Logs go to stderr. |
| `--hidden` | off | Include hidden files and directories in `list` and `search`. |
| `--no-ignore` | off | Do not honour `.gitignore`, `.ignore` and git exclude files. |
| `--max-file-bytes N` | `1048576` | Largest file `read` returns whole, `search` opens, `stat` counts lines in, and `write` accepts. |
| `--max-results N` | `200` | Most entries `list` returns and matches `search` returns. |
| `--allowed-host HOST` | see below | Accepted `Host` header, repeatable. |

`RUST_LOG` sets the log level (default `info`).

## Connect

**Claude Code**

```bash
claude mcp add --transport http folder http://127.0.0.1:8031/mcp     # server already running
claude mcp add folder -- emperor-folder serve --stdio --root ~/notes  # or let Claude Code start it
```

**Claude Desktop** (`claude_desktop_config.json`, stdio):

```json
{
  "mcpServers": {
    "folder": {
      "command": "/usr/local/bin/emperor-folder",
      "args": ["serve", "--stdio", "--root", "/Users/me/notes"]
    }
  }
}
```

**[kowalski](https://github.com/yarenty/kowalski)** (`config.toml`):

```toml
[[mcp.servers]]
name = "folder"
transport = "http"
url = "http://127.0.0.1:8031/mcp"
```

or over stdio:

```toml
[[mcp.servers]]
name = "folder"
transport = "stdio"
command = ["emperor-folder", "serve", "--stdio", "--root", "/home/me/notes"]
```

Any other MCP client that speaks Streamable HTTP or stdio works the same way.

## Tools

Six tools, each a verb. Every path is relative to the root (`.` or omitted means the root).

| Tool | Arguments | Returns |
|---|---|---|
| `list` | `path?`, `glob?`, `recursive?` (false), `max_entries?` | Entries with path, kind (`file`/`dir`/`symlink`) and size; `truncated` when the limit cut it. |
| `read` | `path`, `start_line?`, `end_line?` (1-based, inclusive) | The text, with a `path lines A-B of N` header. Binary files are reported (`binary: true`, size), never dumped. |
| `search` | `pattern` (regex), `path?`, `glob?`, `ignore_case?` (false) | Matches with path, line number and line (cut at 240 characters); counts of files searched, skipped as binary, skipped as too large. |
| `write` | `path`, `content`, `mode?` (`create` \| `overwrite` \| `append`, default `create`) | Path and bytes written. Creates parent directories. `create` refuses an existing file. |
| `move` | `from`, `to`, `overwrite?` (false) | The new path. Renames or moves inside the root; refuses to replace an existing target unless `overwrite`. |
| `stat` | `path` | Kind, size, modified time (RFC 3339, UTC), read-only flag; for text files within the size limit, line count. |

`list` and `search` use [`ignore`](https://crates.io/crates/ignore), ripgrep's walker:
`.gitignore` (even outside a git checkout), `.ignore` and hidden files are skipped unless
`--no-ignore` / `--hidden` say otherwise, and symlinks are never followed. Regexes use the
[`regex`](https://crates.io/crates/regex) crate: linear-time matching, no catastrophic
backtracking, compiled size capped at 1 MiB. Globs match the root-relative path
(`*.rs` matches `src/main.rs`).

**Output.** File content from `read` is framed as reference data
(`FrameKind::TrustedReference`); listings, matches and metadata are JSON framed as computed
results (`FrameKind::Computed`). The frame's per-call nonce means a file containing a fake
end-of-data marker cannot break out of the block. Refusals come back as tool errors
(`isError: true`) with a message that says what to do instead.

## Security model

What the model **can** do: see and read files under the root that the ignore rules do not
hide; with `--writable`, create, change and rename files under the root.

What it **cannot** do:

- **Leave the root.** Absolute paths (`/etc/passwd`, `C:\` on Windows) are refused, and so is any path with
  a `..` component, even one that would stay inside (`a/../b`). That check gives a clear error;
  the enforcement underneath is cap-std, which resolves every path beneath the root handle
  (`openat2(RESOLVE_BENEATH)` on Linux, component-by-component resolution elsewhere) and refuses
  any resolution that leaves it.
- **Escape through symlinks.** A symlink inside the root that points outside is refused for
  `read`, `write`, `move`, `list` and `search` alike; a symlink that stays inside works. The
  walker never follows links, and every path it yields is re-opened through the root handle, so
  a directory swapped for an escaping link mid-walk is skipped, not read.
- **Write by default.** The server starts read-only; `write` and `move` are listed (so the
  model learns they exist) but answer "the server is read-only" until the operator passes
  `--writable`.
- **Exhaust the host.** Whole-file reads stop at `--max-file-bytes` (line-range reads stream and
  return at most that much), `search` skips larger files, `list` and `search` stop at
  `--max-results`, binary files are never returned.
- **Be reached from a web page.** On a loopback bind the `Host` header must be `localhost`,
  `127.0.0.1` or `::1` (DNS-rebinding protection). On a non-loopback bind the check is off,
  as the [Emperor Profile](https://github.com/yarenty/emperor-mcp/blob/main/PROFILE.md) expects a
  validating ingress in front; `--allowed-host` turns it back on for named hosts.

Not in scope: authentication (put the HTTP endpoint behind your gateway or keep it on
loopback), and hard links (a hard link inside the root to a file outside is the same file; do
not point the server at a folder where untrusted users can create hard links). Other local
processes can still change the folder while the server runs; confinement holds regardless,
because every operation re-resolves beneath the root.

## doctor

```
$ emperor-folder doctor --root ~/notes --writable
emperor-folder 0.1.0 doctor
  [ok  ] root folder     /Users/me/notes
  [ok  ] read access     root can be opened and listed
  [ok  ] write access    probe file created and removed
  [ok  ] listen address  127.0.0.1:8031 is free
  [ok  ] tool budget     6 of at most 8 tools
  [ok  ] files           1234 visible files under the root
all good (0 warning(s))
```

It checks the root exists and is a directory, can be opened and listed, can be written when
`--writable` is set (a probe file is created and removed), the listen address is free (skipped
with `--stdio`), the tool count is within the emperor-mcp budget, and counts the files the
walker sees (a warning above 100 000: listing and search get slow). Exit code 0 when nothing
failed, 1 otherwise, so scripts and container health checks can use it.

## Build

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test          # unit tests + every tool over real HTTP and stdio, escape attempts included
cargo deny check
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md): run the checks, keep the docs in step, and sign off
your commits (DCO, `git commit -s`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
emperor-folder by you shall be dual licensed as above, without any additional terms or conditions.
