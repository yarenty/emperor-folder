# emperor-folder next to other folder-access MCP servers

How emperor-folder 0.1.3 compares with the most popular public MCP servers that give a model
access to files. For the shared background (the 2026-07-28 spec revision, annotations,
EscapeRoute, the "lethal trifecta") and what every emperor server gets from the framework, see
[the emperor-mcp comparison](https://github.com/yarenty/emperor-mcp/blob/main/docs/COMPARISON.md).

Figures for other projects were read on **2026-11-01** from the GitHub API (stars, last
release) and from each project's README and source (tool counts). A **(?)** marks a figure that
could not be confirmed.

## Why this domain is risky

A folder server is the shortest path from "the model read a web page" to "the model read your
SSH keys". The most popular server here, the official `filesystem` server, had two path-escape
bugs in 2025 ([EscapeRoute](https://nvd.nist.gov/vuln/detail/CVE-2025-53109), CVE-2025-53109 and
CVE-2025-53110): it compared path strings, so `/allowed_evil` passed for `/allowed`, and a
symlink led out of the folder. Both were fixed and the server has been hardened since, but its
checks are still made on names, as its README now says. emperor-folder does not check names and
then open: it opens each root once as a [cap-std](https://github.com/bytecodealliance/cap-std)
directory handle and does every operation through it, so the operating system
(`openat2(RESOLVE_BENEATH)` on Linux) refuses anything that resolves outside.

## The table

| Server | Stars | Last release | Language | Tools | How writing is controlled | Notes |
|---|---|---|---|---|---|---|
| **emperor-folder** | new | 0.1.1, 2026-11-01 | Rust | 7 (4 when read-only) | Read-only unless `--writable`; the 3 write tools are not listed otherwise; no delete; `write` and `move` do not replace a file unless asked | cap-std handle per root; several named roots; images as MCP image content; `.gitignore`-aware; Streamable HTTP + stdio; `doctor` |
| [official `filesystem`](https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem) | 91.7k (whole monorepo) | 2.1.0, 2026-10-28 (npm) | TypeScript | 14 | Allowed folders on the command line (Roots deprecated); annotations on every tool, 3 destructive; **no read-only mode** (only a read-only Docker mount) | Path-based checks with hardened writes; `edit_file` takes several edits and can show a dry-run diff; stdio only; serves both protocol eras |
| [Desktop Commander](https://github.com/wonderwhy-er/DesktopCommanderMCP) | 10.2k | v0.3.1, 2026-10-22 | TypeScript | 25 (files, terminal, processes, `write_pdf`) | `allowedDirectories` (empty means the whole disk), a blocked-command list | Its own README: "guardrails, not a sandbox"; with a shell, folder limits do not hold |
| [mark3labs/mcp-filesystem-server](https://github.com/mark3labs/mcp-filesystem-server) | 693 | v0.11.1, 2025-06-22 | Go | 14 (incl. `delete_file`) | Allowed folders | Last commit 2025-11-24 |
| [rust-mcp-filesystem](https://github.com/rust-mcp-stack/rust-mcp-filesystem) | 184 | v0.4.5, 2026-08-29 | Rust | 24 (incl. zip/unzip) | **Read-only by default**, a write flag; single tools can be disabled | Closest in philosophy; in Docker's MCP catalog; Homebrew and npm installers |

## emperor-folder: pros

- **The strongest confinement in the table.** Absolute paths, `..` and symlinks that leave a root
  are refused by the operating-system layer, not by a string comparison. Each root has its own
  handle, so a path cannot get from one root into another except through `move`, which names
  both ends.
- **Read-only by default, and the write tools are not even listed.** A read-only server shows 4
  tools. Only rust-mcp-filesystem is also read-only by default.
- **No delete tool.** `write` refuses to replace a file unless `mode: overwrite`; `move` refuses
  to replace a target unless `overwrite: true`.
- **`edit`** replaces one exact snippet (or every occurrence with `replace_all`), so changing one
  line of a large file does not mean sending the whole file back.
- **Several named roots** (`--root notes=~/notes --root code=~/src/app`) and **images** (PNG, JPEG,
  GIF, WebP up to 5 MiB) returned as MCP image content for vision models.
- **Annotations on every tool**, checked by a test against the live tool list.
- **Search from ripgrep's walker**: `.gitignore`, `.ignore` and hidden files honoured, symlinks
  never followed, linear-time regex with a size cap.
- **Bounded output**: file size, characters per read and results per call are capped, and a cut
  read says where to continue.
- **HTTP as well as stdio**, with `Host` checks on loopback and optional `Origin` checks, so it
  can run as a shared service behind a gateway.
- **One binary with provenance**: release binaries with `SHA256SUMS` and build provenance
  attestations, on crates.io, a Dockerfile.

## emperor-folder: cons

- `edit` does one replacement per call and needs an exact match. The official `edit_file` takes
  a list of edits, tolerates whitespace differences and can return a diff without writing.
- No tool to read several files at once and no directory tree: reading five files is five calls.
- No copy, zip or "create an empty directory" tool (`write` creates parent folders).
- Hard links: a hard link inside a root to a file outside is the same file, and confinement does
  not change that. Do not point the server at a folder where untrusted users can create hard
  links. (The official server has the same limit and now reports the link count.)
- No Roots: the folders are set on the command line only. This is deliberate, since the
  2026-07-28 revision deprecates Roots, but a client cannot change the folders at runtime.
- No authentication of its own on HTTP: put it behind your gateway or keep it on loopback.
- New, with no users yet.

## Where a competitor is better

- **The official `filesystem` server**: richer editing (`edit_file` with several edits and a dry
  run), `read_multiple_files`, `directory_tree`, sizes in listings; the reference implementation
  most clients are tested against.
- **rust-mcp-filesystem**: more tools (zip and unzip, duplicate and empty-directory finders,
  head/tail), per-tool disabling, Homebrew and npm installers, already in Docker's catalog.
- **Desktop Commander**: a terminal and process control, which emperor-folder will never have.

## When to pick something else

- You want the agent to run commands, not only touch files: Desktop Commander (and accept that
  the folder limits are then guardrails, not a sandbox).
- You need archives, bulk reads or the exact behaviour of the reference server: the official
  server or rust-mcp-filesystem.
- You want the client to change the allowed folders at runtime through Roots: the official
  server (2025-era clients only).

Pick emperor-folder when the folder boundary has to hold even against a model that has been
talked into trying to leave it, when you want read-only unless you say otherwise, or when the
server runs over HTTP for more than one desktop.
