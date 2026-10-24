use regex::Regex;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn level_line() -> Regex {
    Regex::new(r"^\[rasbari:(debug|info|warn|error)\] [a-z][a-z0-9_]*: .+$").unwrap()
}

/// POPSW-21: every line a core run writes to stderr, which becomes core.log on a POP, starts with a level and a fixed name.
#[test]
fn every_line_of_a_core_run_has_a_level_and_a_name() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("opennow-levels-{unique}"));
    let mut first = Command::new(env!("CARGO_BIN_EXE_opennow-core"))
        .arg("--data-dir")
        .arg(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        writeln!(
            first.stdin.as_mut().unwrap(),
            r#"{{"type":"request","id":"hello","method":"core.hello","params":{{"protocolVersion":6,"shell":"qt"}}}}"#
        )
        .unwrap();
    }
    let stdout = first.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let _ = sender.send(BufReader::new(stdout).read_line(&mut line).map(|_| line));
    });
    let ready = receiver.recv_timeout(Duration::from_secs(10));
    let second = Command::new(env!("CARGO_BIN_EXE_opennow-core"))
        .arg("--data-dir")
        .arg(&directory)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    first.kill().unwrap();
    first.wait().unwrap();
    reader.join().unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    assert!(ready.is_ok(), "the first core never answered");

    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(!lines.is_empty(), "the failed core wrote nothing to stderr");
    let pattern = level_line();
    for line in &lines {
        assert!(pattern.is_match(line), "line without a level: {line}");
    }
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("[rasbari:error] core_failed: ")
                && line.contains("data directory is already in use"))
    );
}

/// POPSW-21: no source file but the logger itself writes to stderr by hand, so a new unleveled line cannot slip in.
#[test]
fn no_source_file_prints_to_stderr_outside_the_logger() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root.clone()];
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path.file_name().is_some_and(|name| name != "rlog.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("eprintln!") || text.contains("eprint!") {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "stderr printed by hand in: {offenders:?}"
    );
}
