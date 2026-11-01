# emperor-folder

[![CI](https://github.com/yarenty/emperor-folder/actions/workflows/ci.yml/badge.svg)](https://github.com/yarenty/emperor-folder/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**An MCP server confined to one folder.** You name a directory (or a few); the model can
list, read, search, and (if you allow it) write, edit and move files inside it, and cannot reach
anything outside it: not with an absolute path, not with `..`, not through a symlink.

```bash
emperor-folder serve --root ~/notes            # read-only, Streamable HTTP on 127.0.0.1:8031/mcp
emperor-folder serve --root ~/notes --writable # also write, edit and move
emperor-folder serve --root ~/notes --stdio    # for desktop clients
emperor-folder serve --root notes=~/notes --root code=~/src/app   # two roots: notes/..., code/...
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
data rather than instructions, and a `doctor` subcommand. It follows
[Emperor Profile P1.1](https://github.com/yarenty/emperor-mcp/blob/main/PROFILE.md): no write
tool is offered unless the operator switches writing on at start-up, and tool definitions are
lean (short descriptions, compact input schemas), so every request carries less of them.

## Install

Release binaries for Linux (x86_64), macOS (Apple Silicon and Intel) and Windows are attached
to each [GitHub release](https://github.com/yarenty/emperor-folder/releases), with a
`SHA256SUMS` file and [build provenance
attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations):
check a download with `sha256sum -c SHA256SUMS --ignore-missing` and
`gh attestation verify <file> --repo yarenty/emperor-folder`. From source (Rust 1.88 or newer):

```bash
cargo install emperor-folder
```

Or as a container (stdio, the folder mounted at `/data`, read-only unless you add `--writable`):

```bash
docker build -t emperor-folder https://github.com/yarenty/emperor-folder.git
docker run -i --rm -v ~/notes:/data emperor-folder                            # read-only
docker run -i --rm -v ~/notes:/data emperor-folder --root /data --writable    # also write, edit, move
docker run -i --rm -v ~/notes:/data -e FOLDER_WRITABLE=true emperor-folder     # the same, from the environment
```

## Run

```bash
emperor-folder doctor --root ~/notes   # check first: exits 1 if something is wrong
emperor-folder serve  --root ~/notes
```

| Flag | Default | Meaning |
|---|---|---|
| `--root [NAME=]DIR` | (required) | A folder the server can see. Repeat for several roots; `NAME=` names one (default: the folder's own name). See [Several roots](#several-roots). |
| `--bind ADDR` | `127.0.0.1:8031` | Streamable HTTP address; MCP endpoint at `/mcp`, liveness at `/health`. |
| `--writable` | off | Offer `write`, `edit` and `move`. Without it they are not listed at all. Also set by the environment variable `FOLDER_WRITABLE`: `true`, `1` or `yes` (also `on`) turn it on; unset, empty, `false`, `0` or `no` (also `off`) leave it off; anything else is refused. |
| `--stdio` | off | Speak MCP over stdin/stdout instead of HTTP. Logs go to stderr. |
| `--hidden` | off | Include hidden files and directories in `list` and `search`. |
| `--no-ignore` | off | Do not honour `.gitignore`, `.ignore` and git exclude files. |
| `--max-file-bytes N` | `1048576` | Largest file `read` returns whole, `search` opens, `stat` counts lines in, and `write` accepts. |
| `--max-results N` | `200` | Most entries `list` returns and matches `search` returns. |
| `--max-chars N` | `50000` | Most characters of text one `read` returns (ceiling 200 000); a longer read stops at a line boundary and says where to continue. |
| `--allowed-host HOST` | see below | Accepted `Host` header, repeatable. |
| `--allowed-origin ORIGIN` | none | Accepted `Origin` header with scheme (`https://app.example.com`), repeatable. Requests without `Origin` (non-browser clients) still pass. |

`RUST_LOG` sets the log level (default `info`).

## Connect

**Claude Code**

```bash
claude mcp add --transport http folder http://127.0.0.1:8031/mcp     # server already running
claude mcp add folder -- emperor-folder serve --stdio --root ~/notes  # or let Claude Code start it
```

With `--writable` (and several roots) in the stdio form:

```bash
claude mcp add folder -- emperor-folder serve --stdio --writable --root notes=$HOME/notes --root code=$HOME/src/app
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

Seven tools, each a verb. Every path is relative to the root (`.` or omitted means the root).
The three that change files are offered only with `--writable`; a read-only server lists four.

| Tool | Arguments | Returns | Annotations |
|---|---|---|---|
| `list` | `path?`, `glob?`, `recursive?` (false), `max_entries?` | Entries with path, kind (`file`/`dir`/`symlink`) and size; `truncated` when the limit cut it. | read-only, idempotent |
| `read` | `path`, `start_line?`, `end_line?` (1-based, inclusive) | The text, with a `path lines A-B of N` header. PNG, JPEG, GIF and WebP images up to 5 MiB (and `--max-file-bytes`) come back as MCP image content; other binary files are reported (`binary: true`, size), never dumped. | read-only, idempotent |
| `search` | `pattern` (regex), `path?`, `glob?`, `ignore_case?` (false) | Matches with path, line number and line (cut at 240 characters); counts of files searched, skipped as binary, skipped as too large. | read-only, idempotent |
| `stat` | `path` | Kind, size, modified time (RFC 3339, UTC), read-only flag; for text files within the size limit, line count; for images, the image type. | read-only, idempotent |
| `write` | `path`, `content`, `mode?` (`create` \| `overwrite` \| `append`, default `create`) | Path and bytes written. Creates parent directories. `create` refuses an existing file. | destructive, `--writable` only |
| `edit` | `path`, `old_text`, `new_text`, `replace_all?` (false) | Path, replacements made, bytes written. `old_text` must occur exactly once (else the error says how many times it occurs) unless `replace_all`. Text files only. | destructive, `--writable` only |
| `move` | `from`, `to`, `overwrite?` (false) | The new path. Renames or moves inside the root (or between roots); refuses to replace an existing target unless `overwrite`. | destructive, `--writable` only |

Every tool sets `openWorldHint: false`: it touches the named folders and nothing else.

`list` and `search` use [`ignore`](https://crates.io/crates/ignore), ripgrep's walker:
`.gitignore` (even outside a git checkout), `.ignore` and hidden files are skipped unless
`--no-ignore` / `--hidden` say otherwise, and symlinks are never followed. Regexes use the
[`regex`](https://crates.io/crates/regex) crate: linear-time matching, no catastrophic
backtracking, compiled size capped at 1 MiB. Globs match the path as the tools show it
(`*.rs` matches `src/main.rs`).

**Output.** File content from `read` is framed as a document of unknown authorship
(`FrameKind::Document`); listings, matches and metadata are JSON framed as computed
results (`FrameKind::Computed`). The frame's per-call nonce means a file containing a fake
end-of-data marker cannot break out of the block. Refusals come back as tool errors
(`isError: true`) with a message that says what to do instead.

## Several roots

`--root` can be given more than once. With one root nothing changes: paths are relative to it.
With several, every path starts with the root's name, and `.` is a virtual folder whose entries
are the roots:

```bash
emperor-folder serve --root notes=~/notes --root code=~/src/app
```

```text
list .                      -> notes, code
read notes/todo.md
search pattern=TODO         -> matches in notes/... and code/...
move  notes/a.md -> code/docs/a.md   (with --writable)
```

A root without a name is named after its folder (`--root ~/notes` is `notes`); two roots with
the same name are refused at start-up. In `NAME=DIR` the name may not contain `/` or `\`, so
`./a=b` is a folder called `a=b`. Each root is its own cap-std handle: a path, `..` or symlink
cannot get from one root into another except through `move`, which names both ends.

## Security model

What the model **can** do: see and read files under the roots that the ignore rules do not
hide; with `--writable`, create, edit and rename files under the roots.

What it **cannot** do:

- **Leave the root.** Absolute paths (`/etc/passwd`, `C:\` on Windows) are refused, and so is any path with
  a `..` component, even one that would stay inside (`a/../b`). That check gives a clear error;
  the enforcement underneath is cap-std, which resolves every path beneath the root handle
  (`openat2(RESOLVE_BENEATH)` on Linux, component-by-component resolution elsewhere) and refuses
  any resolution that leaves it.
- **Escape through symlinks.** A symlink inside a root that points outside it (also into
  another root) is refused for `read`, `write`, `edit`, `move`, `list` and `search` alike; a symlink that stays inside works. The
  walker never follows links, and every path it yields is re-opened through the root handle, so
  a directory swapped for an escaping link mid-walk is skipped, not read.
- **Write by default.** The server starts read-only: `write`, `edit` and `move` are removed
  from the tool list, and the file operations refuse writes as well, until the operator passes
  `--writable` (or sets `FOLDER_WRITABLE`), as Emperor Profile P1.1 requires. Tool
  annotations (`readOnlyHint`, `destructiveHint`) tell clients which tools change files, so
  they can ask before running them.
- **Exhaust the host or the context.** Whole-file reads stop at `--max-file-bytes` (line-range
  reads stream and return at most that much) and at `--max-chars` characters, `search` skips
  larger files, `list` and `search` stop at `--max-results`, binary files other than small
  images are never returned.
- **Be reached from a web page.** On a loopback bind the `Host` header must be `localhost`,
  `127.0.0.1` or `::1` (DNS-rebinding protection). On a non-loopback bind the check is off,
  as the [Emperor Profile](https://github.com/yarenty/emperor-mcp/blob/main/PROFILE.md) expects a
  validating ingress in front; `--allowed-host` turns it back on for named hosts, and
  `--allowed-origin` refuses browser requests from any other origin.

Not in scope: authentication (put the HTTP endpoint behind your gateway or keep it on
loopback), and hard links (a hard link inside the root to a file outside is the same file; do
not point the server at a folder where untrusted users can create hard links). Other local
processes can still change the folder while the server runs; confinement holds regardless,
because every operation re-resolves beneath the root.

## doctor

```
$ emperor-folder doctor --root ~/notes --writable
emperor-folder 0.1.3 doctor
  [ok  ] dir /Users/me/notes  readable (42 entries)
  [ok  ] dir /Users/me/notes  writable
  [ok  ] write access         on: write, edit and move are offered
  [ok  ] roots                notes = /Users/me/notes
  [ok  ] listen address       127.0.0.1:8031 is free
  [ok  ] tool budget          7 of at most 8 tools
  [ok  ] files                1234 visible files under the roots
all good (0 warning(s))
```

It checks each root exists and can be listed, can be written when `--writable` is set (a probe
file is created and removed), says whether writing is on, the roots open and have distinct names, the listen address is free (skipped
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

## Links

- Crate: [crates.io/crates/emperor-folder](https://crates.io/crates/emperor-folder), API docs on [docs.rs](https://docs.rs/emperor-folder)
- Framework: [emperor-mcp](https://github.com/yarenty/emperor-mcp)
- How it compares with other folder-access MCP servers: [docs/COMPARISON.md](docs/COMPARISON.md)
- Official [MCP Registry](https://registry.modelcontextprotocol.io/) entry, mcp-name: io.github.yarenty/emperor-folder

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
