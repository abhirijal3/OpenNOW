//! Per-packet and per-frame timing trace for diagnosing stutter and black screens.
//!
//! Every stage of the video path (packet arrival, FEC repair, NACK, assembly,
//! decoder queue, VideoToolbox decode, render-thread acquire) appends one CSV
//! line here, all on one monotonic microsecond clock, so a frame can be
//! followed end to end offline. `tools/frame-trace/report.mjs` reads the file.
//!
//! On by default in this build; set `OPENNOW_FRAME_TRACE=0` to turn it off and
//! `OPENNOW_FRAME_TRACE_DIR` to choose where files go. Lines are handed to a
//! writer thread through a bounded queue: a slow disk drops trace lines (and
//! says how many), it never blocks the receive or decode threads.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bumped whenever a record's fields change, so the report can refuse old files.
pub const TRACE_VERSION: u32 = 1;

/// About ten seconds of packets at 100 Mbps; the writer drains far faster.
const QUEUE_LINES: usize = 1 << 17;
const FLUSH_EVERY: Duration = Duration::from_millis(250);

static ENABLED: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU64 = AtomicU64::new(0);

enum Message {
    Open(PathBuf, String),
    Line(String),
}

struct Clock {
    origin: Instant,
    unix_us_at_origin: u64,
}

fn clock() -> &'static Clock {
    static CLOCK: OnceLock<Clock> = OnceLock::new();
    CLOCK.get_or_init(|| Clock {
        origin: Instant::now(),
        unix_us_at_origin: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros() as u64)
            .unwrap_or(0),
    })
}

fn sender() -> &'static Option<SyncSender<Message>> {
    static SENDER: OnceLock<Option<SyncSender<Message>>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, receiver) = sync_channel::<Message>(QUEUE_LINES);
        std::thread::Builder::new()
            .name("opennow-frame-trace".to_owned())
            .spawn(move || {
                let mut out: Option<BufWriter<File>> = None;
                loop {
                    match receiver.recv_timeout(FLUSH_EVERY) {
                        Ok(Message::Open(path, header)) => {
                            if let Some(mut previous) = out.take() {
                                let _ = previous.flush();
                            }
                            out = File::create(&path)
                                .ok()
                                .map(|file| BufWriter::with_capacity(1 << 20, file));
                            if let Some(writer) = out.as_mut() {
                                let _ = writer.write_all(header.as_bytes());
                            }
                        }
                        Ok(Message::Line(line)) => {
                            if let Some(writer) = out.as_mut() {
                                let _ = writer.write_all(line.as_bytes());
                                let _ = writer.write_all(b"\n");
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some(writer) = out.as_mut() {
                                let _ = writer.flush();
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    let dropped = DROPPED.swap(0, Ordering::Relaxed);
                    if dropped > 0
                        && let Some(writer) = out.as_mut()
                    {
                        let _ = writeln!(writer, "W,{},{dropped}", now_us());
                    }
                }
                if let Some(mut writer) = out {
                    let _ = writer.flush();
                }
            })
            .ok()
            .map(|_| sender)
    })
}

fn trace_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OPENNOW_FRAME_TRACE_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "macos")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home)
            .join("Library/Application Support/OpenNOW/diagnostics/frame-traces");
    }
    std::env::temp_dir().join("opennow-frame-traces")
}

/// True while a trace file is open. Call sites check this before formatting.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Microseconds since the trace clock's origin.
pub fn now_us() -> u64 {
    us_at(Instant::now())
}

/// An `Instant` on the trace clock. Instants from before the origin read 0.
pub fn us_at(instant: Instant) -> u64 {
    instant
        .saturating_duration_since(clock().origin)
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Starts a new trace file for a stream and returns its path. Later lines go
/// to it; any earlier file is flushed and closed in order.
pub fn start_session(label: &str) -> Option<PathBuf> {
    if std::env::var("OPENNOW_FRAME_TRACE").is_ok_and(|value| value == "0") {
        ENABLED.store(false, Ordering::Relaxed);
        return None;
    }
    let clock = clock();
    let dir = trace_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("frame-trace-{started_unix_ms}.csv"));
    let header = format!(
        "H,{},{},{TRACE_VERSION},{}\n",
        now_us(),
        clock.unix_us_at_origin,
        clean(label)
    );
    let sender = sender().as_ref()?;
    sender.send(Message::Open(path.clone(), header)).ok()?;
    ENABLED.store(true, Ordering::Relaxed);
    Some(path)
}

/// Queues one record. Never blocks; a full queue counts a drop instead.
pub fn emit(line: String) {
    if !enabled() {
        return;
    }
    if let Some(sender) = sender().as_ref()
        && sender.try_send(Message::Line(line)).is_err()
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Makes free text safe for one CSV field: no commas, no line breaks.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            ',' => ';',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_keeps_one_field() {
        assert_eq!(clean("a,b\nc"), "a;b c");
    }

    #[test]
    fn instants_before_origin_read_zero() {
        let before = Instant::now();
        let _ = clock();
        assert!(us_at(before) <= now_us());
    }
}
