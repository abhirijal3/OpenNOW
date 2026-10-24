use opennow_streamer_protocol::rlog::{Level, Limiter, format_line};
use std::path::{Path, PathBuf};

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// POPSW-21: no source file in the streamer crates prints to stderr by hand, so every line v2pop gets from the streamer has a level and a fixed name.
#[test]
fn no_streamer_source_prints_to_stderr_outside_the_logger() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if name == "rlog.rs" || name == "leveled_stderr.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        if text.contains("eprintln!") || text.contains("eprint!") {
            offenders.push(file.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "stderr printed by hand in: {offenders:?}"
    );
}

/// POPSW-21: the line the streamer writes is the same shape the POP controller's parser reads, and a burst is capped.
#[test]
fn a_streamer_line_has_the_shared_shape_and_a_burst_is_capped() {
    let line = format_line(
        Level::Warn,
        "udp_packet_oversized",
        "error=too big next=drop_packet",
    );
    assert_eq!(
        line,
        "[rasbari:warn] udp_packet_oversized: error=too big next=drop_packet"
    );
    let mut limiter = Limiter::default();
    let mut printed = 0;
    for _ in 0..400 {
        printed += limiter
            .offer(3, Level::Warn, "udp_packet_oversized", line.clone())
            .len();
    }
    assert_eq!(printed, 10);
    let next = limiter.offer(4, Level::Info, "x", format_line(Level::Info, "x", "a=1"));
    assert_eq!(
        next[0],
        "[rasbari:warn] udp_packet_oversized: repeated 390 more times in 1 s"
    );
}
