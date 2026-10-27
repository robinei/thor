//! ClamShell's logger: each record a line on stderr, `[time LEVEL target] message`, the time in UTC to the
//! second; to the journal (as a service), which keeps the time itself, without it, and starting with its
//! syslog priority, which journald takes as the entry's, so that `journalctl -p err` finds the errors. `RUST_LOG` filters as env_logger's plain form does: comma-separated
//! `level` for everything and `target=level` for a module and those under it, the most specific winning;
//! without it, `info,tracing=warn`.

use std::io::Write;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Logs through this logger from now on, to the journal's form if `journal`.
pub fn init(journal: bool) {
    let filter = Filter::parse(std::env::var("RUST_LOG").ok().as_deref().unwrap_or("info,tracing=warn"));
    log::set_max_level(filter.most());
    if log::set_boxed_logger(Box::new(Logger { filter, journal })).is_err() {
        eprintln!("logging: a logger was already set");
    }
}

struct Logger {
    filter: Filter,
    journal: bool,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = if self.journal {
            let priority = match record.level() {
                Level::Error => 3,
                Level::Warn => 4,
                Level::Info => 6,
                Level::Debug | Level::Trace => 7,
            };
            format!("<{priority}>[{:<5} {}] {}\n", record.level(), record.target(), record.args())
        } else {
            let time = jiff::Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ");
            format!("[{time} {:<5} {}] {}\n", record.level(), record.target(), record.args())
        };
        // One write, so that lines from threads don't interleave.
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// What `RUST_LOG` lets through: a level for everything, and levels for modules.
#[derive(Debug, PartialEq)]
struct Filter {
    all: LevelFilter,
    modules: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// `RUST_LOG`'s plain form; a part it can't read is left out, with a word on stderr.
    fn parse(text: &str) -> Filter {
        let mut filter = Filter { all: LevelFilter::Error, modules: Vec::new() };
        for part in text.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            match part.split_once('=') {
                Some((module, level)) => match level.trim().parse() {
                    Ok(level) => filter.modules.push((module.trim().to_owned(), level)),
                    Err(_) => eprintln!("logging: RUST_LOG's {part:?} has no level"),
                },
                None => match part.parse() {
                    Ok(level) => filter.all = level,
                    // A module named alone is let through whole, as env_logger does.
                    Err(_) => filter.modules.push((part.to_owned(), LevelFilter::Trace)),
                },
            }
        }
        filter
    }

    /// The level let through for `target`: its most specific module's (the longest whose name it is or
    /// starts with, followed by `::`), else the level for everything.
    fn level(&self, target: &str) -> LevelFilter {
        let under = |module: &str| target == module || target.strip_prefix(module).is_some_and(|rest| rest.starts_with("::"));
        self.modules.iter().filter(|(module, _)| under(module)).max_by_key(|(module, _)| module.len()).map_or(self.all, |(_, level)| *level)
    }

    /// The most it lets through anywhere, for the log crate to skip anything above it at once.
    fn most(&self) -> LevelFilter {
        self.modules.iter().map(|(_, level)| *level).fold(self.all, Ord::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_log_lets_through_a_level_and_modules_the_most_specific_winning() {
        let filter = Filter::parse("info,tracing=warn,clamshell_compositor::x11=debug,clamshell_compositor::x11::wm=trace");
        assert_eq!(filter.level("clamshell"), LevelFilter::Info);
        assert_eq!(filter.level("tracing"), LevelFilter::Warn);
        assert_eq!(filter.level("tracing::span"), LevelFilter::Warn);
        assert_eq!(filter.level("tracing_core"), LevelFilter::Info, "a module's name, not a prefix of one");
        assert_eq!(filter.level("clamshell_compositor::x11::shell"), LevelFilter::Debug);
        assert_eq!(filter.level("clamshell_compositor::x11::wm"), LevelFilter::Trace);
        assert_eq!(filter.level("clamshell_compositor::drm"), LevelFilter::Info);
        assert_eq!(filter.most(), LevelFilter::Trace);
    }

    #[test]
    fn rust_log_with_no_level_for_everything_lets_errors_through() {
        let filter = Filter::parse("smithay=debug");
        assert_eq!(filter.level("clamshell"), LevelFilter::Error);
        assert_eq!(filter.level("smithay::backend"), LevelFilter::Debug);
        assert_eq!(Filter::parse("").level("anything"), LevelFilter::Error);
        assert_eq!(Filter::parse("warn").level("anything"), LevelFilter::Warn);
    }
}
