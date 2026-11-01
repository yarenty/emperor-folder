//! The binary itself: `doctor` exit codes and a `serve --stdio` round trip.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_emperor-folder");

#[test]
fn doctor_passes_on_a_good_root_and_fails_on_a_missing_one() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "a").unwrap();

    let ok = Command::new(BIN)
        .args(["doctor", "--stdio", "--writable", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&ok.stdout);
    assert!(ok.status.success(), "{out}");
    assert!(
        out.contains("tool budget") && out.contains("7 of at most 8"),
        "{out}"
    );
    assert!(out.contains("writable"), "{out}");
    assert!(out.contains("1 visible files"), "{out}");
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        1,
        "probe removed"
    );

    let missing = Command::new(BIN)
        .args(["doctor", "--stdio", "--root"])
        .arg(root.path().join("nope"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("[FAIL] dir "));
}

#[test]
fn doctor_fails_when_the_port_is_taken() {
    let root = tempfile::tempdir().unwrap();
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = busy.local_addr().unwrap().to_string();
    let out = Command::new(BIN)
        .args(["doctor", "--bind", &addr, "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("[FAIL] listen address"));
}

#[test]
fn serve_over_stdio_lists_tools_and_reads_a_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), "hello from stdio\n").unwrap();

    let mut child = Command::new(BIN)
        .args(["serve", "--stdio", "--root"])
        .arg(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let reply = |id: i64| loop {
        let v = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("stdio reply");
        if v["id"] == id {
            return v;
        }
    };

    send(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"test","version":"0"}}}),
    );
    let init = reply(1);
    assert_eq!(
        init["result"]["serverInfo"]["name"], "emperor-folder",
        "{init}"
    );
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = reply(2);
    assert_eq!(
        list["result"]["tools"].as_array().unwrap().len(),
        4,
        "read-only: write, edit and move are not listed: {list}"
    );

    send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"read","arguments":{"path":"hello.txt"}}}));
    let read = reply(3);
    let text = read["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("hello from stdio") && text.contains("BEGIN_DATA_"),
        "{read}"
    );

    drop(send);
    let _ = child.kill();
    let _ = child.wait();
}
