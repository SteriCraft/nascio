//! The server's output lines: printed on the terminal and kept (the last [`KEPT`]) for the
//! console of the window, which reads them by number with [`since`].

use std::collections::VecDeque;
use std::sync::Mutex;

/// Lines kept for the window.
pub const KEPT: usize = 5_000;

struct Log {
    lines: VecDeque<String>,
    /// Number of the first kept line.
    first: u64,
}

static LOG: Mutex<Log> = Mutex::new(Log {
    lines: VecDeque::new(),
    first: 0,
});

fn keep(s: &str) {
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    for l in s.lines() {
        log.lines.push_back(l.to_owned());
    }
    while log.lines.len() > KEPT {
        log.lines.pop_front();
        log.first += 1;
    }
}

/// Prints a line (standard output) and keeps it.
pub fn out(s: String) {
    println!("{s}");
    keep(&s);
}

/// Prints a line on the error output and keeps it.
pub fn err(s: String) {
    eprintln!("{s}");
    keep(&s);
}

/// The lines from number `next` on, and the number after the last one. Lines dropped before
/// they were read are skipped.
pub fn since(next: u64) -> (Vec<String>, u64) {
    let log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let end = log.first + log.lines.len() as u64;
    let from = next.max(log.first);
    let lines = log
        .lines
        .iter()
        .skip((from - log.first) as usize)
        .cloned()
        .collect();
    (lines, end)
}

/// `println!` that the window's console also shows.
#[macro_export]
macro_rules! say {
    ($($t:tt)*) => { $crate::console::out(format!($($t)*)) };
}

/// `eprintln!` that the window's console also shows.
#[macro_export]
macro_rules! say_err {
    ($($t:tt)*) => { $crate::console::err(format!($($t)*)) };
}
