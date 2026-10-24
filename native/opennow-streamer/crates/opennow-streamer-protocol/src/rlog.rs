//! Leveled stderr lines for the streamer: `[rasbari:<level>] <name>: <text>`.
//!
//! `<level>` is debug, info, warn or error, `<name>` is a fixed snake_case event name and
//! `<text>` holds the changing facts as `key=value` pairs. The same format is written by the
//! protocol library's `v2/src/rlog.c` and the core's `rlog.rs`, so one parser in the POP
//! controller reads all of them. Debug lines (per-message and per-interval narration) are only
//! printed when `OPENNOW_NVST_TRACE` is on, so a stream never pays for a line nobody asked for. A line printed more than [`FULL_PER_SECOND`] times in one
//! second is printed that many times and then replaced by one
//! `<name>: repeated N more times in 1 s` line (Abhi, 2026-10-24). The streamer runs inside
//! `v2pop`, so these lines land in `v2pop.log`.
//! @rule POPSW-21

use std::collections::HashMap;
use std::fmt::Arguments;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub const FULL_PER_SECOND: u32 = 10;
const MAX_TEXT: usize = 2048;
const MAX_SLOTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// Builds one line, with the text cut at 2048 characters and any newline in it turned into a space.
pub fn format_line(level: Level, name: &str, text: &str) -> String {
    let flat: String = text
        .chars()
        .take(MAX_TEXT)
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    format!("[rasbari:{}] {}: {}", level.as_str(), name, flat.trim_end())
}

struct Slot {
    second: u64,
    count: u32,
    level: Level,
    name: String,
}

/// The burst limiter, with time passed in so a test can drive it.
#[derive(Default)]
pub struct Limiter {
    slots: HashMap<String, Slot>,
    last_second: u64,
}

impl Limiter {
    /// Offers one line at `second` and returns the lines to print now: the line itself while it is
    /// among the first ten identical ones of the second, and the counted line of any burst that has ended.
    pub fn offer(&mut self, second: u64, level: Level, name: &str, line: String) -> Vec<String> {
        let mut out = Vec::new();
        if second != self.last_second {
            self.last_second = second;
            let stale: Vec<String> = self
                .slots
                .iter()
                .filter(|(_, slot)| slot.second != second)
                .map(|(key, _)| key.clone())
                .collect();
            for key in stale {
                if let Some(slot) = self.slots.remove(&key) {
                    out.extend(counted(&slot));
                }
            }
        }
        if !self.slots.contains_key(&line) && self.slots.len() >= MAX_SLOTS {
            if let Some(oldest) = self.slots.keys().next().cloned() {
                if let Some(slot) = self.slots.remove(&oldest) {
                    out.extend(counted(&slot));
                }
            }
        }
        let slot = self.slots.entry(line.clone()).or_insert(Slot {
            second,
            count: 0,
            level,
            name: name.to_owned(),
        });
        slot.count += 1;
        if slot.count <= FULL_PER_SECOND {
            out.push(line);
        }
        out
    }

    /// Returns the counted line of every burst still open, which is what a program does as it exits.
    pub fn flush(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for (_, slot) in self.slots.drain() {
            out.extend(counted(&slot));
        }
        out
    }
}

fn counted(slot: &Slot) -> Option<String> {
    if slot.count <= FULL_PER_SECOND {
        return None;
    }
    Some(format_line(
        slot.level,
        &slot.name,
        &format!(
            "repeated {} more times in 1 s",
            slot.count - FULL_PER_SECOND
        ),
    ))
}

fn state() -> &'static Mutex<(Limiter, Instant)> {
    static STATE: OnceLock<Mutex<(Limiter, Instant)>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new((Limiter::default(), Instant::now())))
}

fn write_all(lines: &[String]) {
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    for line in lines {
        let _ = writeln!(handle, "{line}");
    }
}

/// Writes one leveled line to stderr through the burst limiter.
pub fn emit(level: Level, name: &str, text: Arguments) {
    let line = format_line(level, name, &text.to_string());
    let lines = match state().lock() {
        Ok(mut guard) => {
            let second = guard.1.elapsed().as_secs();
            guard.0.offer(second, level, name, line)
        }
        Err(_) => vec![line],
    };
    write_all(&lines);
}

/// Writes the counted line of every open burst. Called before the core exits.
pub fn flush() {
    if let Ok(mut guard) = state().lock() {
        let lines = guard.0.flush();
        write_all(&lines);
    }
}

/// Whether lines of this level are printed: all of them, except debug, which needs `OPENNOW_NVST_TRACE`.
pub fn enabled(level: Level) -> bool {
    if level != Level::Debug {
        return true;
    }
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        std::env::var("OPENNOW_NVST_TRACE")
            .ok()
            .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
    })
}

#[macro_export]
macro_rules! rlog {
    ($level:ident, $name:literal, $($arg:tt)+) => {
        if $crate::rlog::enabled($crate::rlog::Level::$level) {
            $crate::rlog::emit($crate::rlog::Level::$level, $name, format_args!($($arg)+))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> String {
        format_line(Level::Error, "decrypt_failed", text)
    }

    /// POPSW-21: a line has its level, its fixed name and its text, on one line.
    #[test]
    fn a_line_carries_level_name_and_text_on_one_line() {
        assert_eq!(
            format_line(
                Level::Warn,
                "provider_fallback_used",
                "provider=NVIDIA\nfallback=x"
            ),
            "[rasbari:warn] provider_fallback_used: provider=NVIDIA fallback=x"
        );
        assert_eq!(Level::Debug.as_str(), "debug");
        assert_eq!(
            format_line(Level::Info, "a", &"x".repeat(5000)).len(),
            "[rasbari:info] a: ".len() + 2048
        );
    }

    /// POPSW-21: 400 identical lines in one second print 10 and then one count of 390 once the second is over.
    #[test]
    fn four_hundred_identical_lines_print_ten_and_one_count() {
        let mut limiter = Limiter::default();
        let mut printed = Vec::new();
        for _ in 0..400 {
            printed.extend(limiter.offer(5, Level::Error, "decrypt_failed", line("stream=video")));
        }
        assert_eq!(printed.len(), 10);
        let after = limiter.offer(
            6,
            Level::Info,
            "other",
            format_line(Level::Info, "other", "x=1"),
        );
        assert_eq!(after.len(), 2);
        assert_eq!(
            after[0],
            "[rasbari:error] decrypt_failed: repeated 390 more times in 1 s"
        );
        assert_eq!(after[1], "[rasbari:info] other: x=1");
    }

    /// POPSW-21: flush at exit reports a burst that never saw a later second, and nothing for a burst of 10 or fewer.
    #[test]
    fn flush_counts_an_open_burst_and_ignores_a_small_one() {
        let mut limiter = Limiter::default();
        for _ in 0..15 {
            limiter.offer(
                1,
                Level::Warn,
                "same",
                format_line(Level::Warn, "same", "v=1"),
            );
        }
        for _ in 0..10 {
            limiter.offer(
                1,
                Level::Warn,
                "small",
                format_line(Level::Warn, "small", "v=1"),
            );
        }
        assert_eq!(
            limiter.flush(),
            vec!["[rasbari:warn] same: repeated 5 more times in 1 s".to_owned()]
        );
        assert!(limiter.flush().is_empty());
    }

    /// POPSW-21: the next second starts a fresh count.
    #[test]
    fn the_next_second_starts_a_fresh_count() {
        let mut limiter = Limiter::default();
        let mut printed = 0;
        for second in [1, 2] {
            for _ in 0..20 {
                printed += limiter
                    .offer(
                        second,
                        Level::Info,
                        "x",
                        format_line(Level::Info, "x", "v=1"),
                    )
                    .iter()
                    .filter(|l| !l.contains("repeated"))
                    .count();
            }
        }
        assert_eq!(printed, 20);
    }

    /// POPSW-21: more distinct lines than slots does not grow the table without bound.
    #[test]
    fn the_table_of_lines_is_bounded() {
        let mut limiter = Limiter::default();
        for i in 0..(MAX_SLOTS * 3) {
            limiter.offer(
                1,
                Level::Info,
                "x",
                format_line(Level::Info, "x", &format!("i={i}")),
            );
        }
        assert!(limiter.slots.len() <= MAX_SLOTS);
    }
}
