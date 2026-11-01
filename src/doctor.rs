//! `emperor-folder doctor`: checks the root folder, the listen address and the tool budget
//! before the server is started.

use std::net::SocketAddr;

use emperor_mcp::doctor::{Check, Report, port_free, tool_budget};

use crate::folder::{Config, Folder};
use crate::server::{FolderServer, SERVER_NAME};

/// Above this many files under the root, `doctor` warns: listing and search get slow.
pub const FILE_COUNT_WARN: usize = 100_000;

/// Build the report. `bind` is `None` for stdio (no port to check).
pub fn report(config: &Config, bind: Option<SocketAddr>) -> Report {
    let mut report = Report::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
    let shown = config
        .root
        .canonicalize()
        .unwrap_or_else(|_| config.root.clone())
        .display()
        .to_string();

    match std::fs::metadata(&config.root) {
        Ok(md) if md.is_dir() => {
            report.push(Check::pass("root folder", shown.clone()));
        }
        Ok(_) => {
            report.push(Check::fail(
                "root folder",
                format!("{shown} is not a directory"),
            ));
        }
        Err(e) => {
            report.push(Check::fail("root folder", format!("{shown}: {e}")));
        }
    }

    let folder = match Folder::open(config.clone()) {
        Ok(f) => match f.dir().entries() {
            Ok(_) => {
                report.push(Check::pass("read access", "root can be opened and listed"));
                Some(f)
            }
            Err(e) => {
                report.push(Check::fail("read access", format!("cannot list root: {e}")));
                None
            }
        },
        Err(e) => {
            report.push(Check::fail("read access", e.to_string()));
            None
        }
    };

    if !config.writable {
        report.push(Check::pass(
            "write access",
            "read-only (write and move answer an error; --writable enables them)",
        ));
    } else if let Some(f) = &folder {
        report.push(write_probe(f));
    } else {
        report.push(Check::fail("write access", "root cannot be opened"));
    }

    match bind {
        Some(addr) => report.push(port_free(addr)),
        None => report.push(Check::pass("listen address", "stdio, no port")),
    };

    report.push(tool_budget(FolderServer::tool_count()));

    if let Some(f) = &folder {
        let n = f.count_files(FILE_COUNT_WARN);
        report.push(if n > FILE_COUNT_WARN {
            Check::warn(
                "files",
                format!(
                    "more than {FILE_COUNT_WARN} files under the root; list and search will be slow, consider a narrower root"
                ),
            )
        } else {
            Check::pass("files", format!("{n} visible files under the root"))
        });
    }
    report
}

/// Create and remove a probe file through the capability handle.
fn write_probe(folder: &Folder) -> Check {
    let name = format!(".emperor-folder-doctor-{}", std::process::id());
    match folder.dir().write(&name, b"probe") {
        Ok(()) => {
            let _ = folder.dir().remove_file(&name);
            Check::pass("write access", "probe file created and removed")
        }
        Err(e) => Check::fail("write access", format!("cannot write in root: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emperor_mcp::doctor::Status;

    #[test]
    fn missing_root_fails() {
        let r = report(&Config::new("/definitely/not/here/emperor-folder"), None);
        assert!(!r.is_ok());
        assert_eq!(r.checks()[0].status, Status::Fail);
    }
}
