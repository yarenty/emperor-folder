//! `emperor-folder doctor`: checks the root folders, the listen address and the tool budget
//! before the server is started.

use std::net::SocketAddr;

use emperor_mcp::doctor::{Check, Report, dir_readable, dir_writable, port_free, tool_budget};

use crate::folder::{Config, Folder};
use crate::server::{FolderServer, SERVER_NAME};

/// Above this many files under the roots, `doctor` warns: listing and search get slow.
pub const FILE_COUNT_WARN: usize = 100_000;

/// Build the report. `bind` is `None` for stdio (no port to check).
pub fn report(config: &Config, bind: Option<SocketAddr>) -> Report {
    let mut report = Report::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));

    for spec in &config.roots {
        report.push(dir_readable(&spec.path));
        if config.writable {
            report.push(dir_writable(&spec.path));
        }
    }
    report.push(Check::pass(
        "write access",
        if config.writable {
            "on: write, edit and move are offered"
        } else {
            "read-only: write, edit and move are not offered (--writable or FOLDER_WRITABLE=true enables them)"
        },
    ));

    let folder = match Folder::open(config.clone()) {
        Ok(f) => {
            let roots: Vec<String> = f
                .roots()
                .map(|(name, path)| format!("{name} = {}", path.display()))
                .collect();
            report.push(Check::pass("roots", roots.join(", ")));
            Some(f)
        }
        Err(e) => {
            report.push(Check::fail("roots", e.to_string()));
            None
        }
    };

    match bind {
        Some(addr) => report.push(port_free(addr)),
        None => report.push(Check::pass("listen address", "stdio, no port")),
    };

    report.push(tool_budget(FolderServer::tool_count(config.writable)));

    if let Some(f) = &folder {
        let n = f.count_files(FILE_COUNT_WARN);
        report.push(if n > FILE_COUNT_WARN {
            Check::warn(
                "files",
                format!(
                    "more than {FILE_COUNT_WARN} files under the roots; list and search will be slow, consider a narrower root"
                ),
            )
        } else {
            Check::pass("files", format!("{n} visible files under the roots"))
        });
    }
    report
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

    #[test]
    fn duplicate_root_names_fail() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let config = Config::new(a.path())
            .root("x", b.path())
            .root("x", a.path());
        let r = report(&config, None);
        assert!(
            r.checks()
                .iter()
                .any(|c| c.name == "roots" && c.status == Status::Fail)
        );
    }
}
