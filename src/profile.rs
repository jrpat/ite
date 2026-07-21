//! Lightweight span profiler. Enabled by setting `ITE_PROFILE=<output-path>`;
//! when unset, spans cost one branch. The dump is a plain-text table.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Collects labeled durations and renders a summary table.
pub struct Registry {
    records: Mutex<Vec<(&'static str, Duration)>>,
}

impl Registry {
    pub const fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
        }
    }

    pub fn record(&self, label: &'static str, duration: Duration) {
        self.records.lock().unwrap().push((label, duration));
    }

    /// A table of per-label stats, ordered by total time descending.
    pub fn summary(&self) -> String {
        let records = self.records.lock().unwrap();
        if records.is_empty() {
            return "(no spans recorded)\n".to_string();
        }
        let mut by_label: Vec<(&'static str, Vec<Duration>)> = Vec::new();
        for &(label, duration) in records.iter() {
            match by_label.iter_mut().find(|(l, _)| *l == label) {
                Some((_, durations)) => durations.push(duration),
                None => by_label.push((label, vec![duration])),
            }
        }
        let mut rows: Vec<(&'static str, Stats)> = by_label
            .into_iter()
            .map(|(label, durations)| (label, Stats::from_durations(&durations).unwrap()))
            .collect();
        rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.total));

        let mut out = format!(
            "{:<24} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9}\n",
            "span", "count", "total", "mean", "p50", "p95", "max"
        );
        for (label, s) in rows {
            out.push_str(&format!(
                "{:<24} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9}\n",
                label,
                s.count,
                format_duration(s.total),
                format_duration(s.mean),
                format_duration(s.p50),
                format_duration(s.p95),
                format_duration(s.max),
            ));
        }
        out
    }

    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.summary())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

pub static GLOBAL: Registry = Registry::new();

/// The output path from `ITE_PROFILE`, if profiling is enabled.
pub fn output_path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| std::env::var("ITE_PROFILE").ok().filter(|p| !p.is_empty()))
        .as_deref()
}

pub fn enabled() -> bool {
    output_path().is_some()
}

/// Times a scope and records it in [`GLOBAL`] on drop (no-op when disabled).
pub struct Span {
    label: &'static str,
    start: Option<Instant>,
}

#[must_use]
pub fn span(label: &'static str) -> Span {
    Span {
        label,
        start: enabled().then(Instant::now),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            GLOBAL.record(self.label, start.elapsed());
        }
    }
}

/// Summary statistics over a set of durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub count: usize,
    pub total: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

impl Stats {
    /// `None` when `durations` is empty.
    pub fn from_durations(durations: &[Duration]) -> Option<Self> {
        if durations.is_empty() {
            return None;
        }
        let mut sorted = durations.to_vec();
        sorted.sort();
        let percentile = |q: f64| {
            let index = ((sorted.len() - 1) as f64 * q).round() as usize;
            sorted[index]
        };
        let total: Duration = sorted.iter().sum();
        Some(Self {
            count: sorted.len(),
            total,
            mean: total / sorted.len() as u32,
            p50: percentile(0.50),
            p95: percentile(0.95),
            max: *sorted.last().unwrap(),
        })
    }
}

/// Adaptive human-readable duration: ns, µs, ms, or s.
pub fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos < 1_000 {
        format!("{nanos}ns")
    } else if nanos < 1_000_000 {
        format!("{:.1}µs", nanos as f64 / 1_000.0)
    } else if nanos < 1_000_000_000 {
        format!("{:.2}ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", nanos as f64 / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn stats_of_known_durations() {
        let durations: Vec<Duration> = (1..=10).map(ms).collect();
        let stats = Stats::from_durations(&durations).unwrap();
        assert_eq!(stats.count, 10);
        assert_eq!(stats.total, ms(55));
        assert_eq!(stats.mean, Duration::from_micros(5500));
        assert_eq!(stats.max, ms(10));
        // Percentile index = round((n-1) * q) over the sorted set.
        assert_eq!(stats.p50, ms(6));
        assert_eq!(stats.p95, ms(10));
    }

    #[test]
    fn stats_of_single_duration() {
        let stats = Stats::from_durations(&[ms(10)]).unwrap();
        assert_eq!(stats.count, 1);
        assert_eq!(stats.mean, ms(10));
        assert_eq!(stats.p50, ms(10));
        assert_eq!(stats.p95, ms(10));
        assert_eq!(stats.max, ms(10));
    }

    #[test]
    fn stats_of_nothing_is_none() {
        assert_eq!(Stats::from_durations(&[]), None);
    }

    #[test]
    fn stats_do_not_require_sorted_input() {
        let stats = Stats::from_durations(&[ms(9), ms(1), ms(5)]).unwrap();
        assert_eq!(stats.p50, ms(5));
        assert_eq!(stats.max, ms(9));
    }

    #[test]
    fn summary_orders_labels_by_total_descending() {
        let reg = Registry::new();
        reg.record("cheap", ms(2));
        reg.record("expensive", ms(50));
        reg.record("cheap", ms(3));
        let summary = reg.summary();
        let expensive_at = summary.find("expensive").unwrap();
        let cheap_at = summary.find("cheap").unwrap();
        assert!(expensive_at < cheap_at, "summary:\n{summary}");
        assert!(summary.contains("count"), "has a header:\n{summary}");
    }

    #[test]
    fn summary_reports_counts() {
        let reg = Registry::new();
        reg.record("draw", ms(1));
        reg.record("draw", ms(1));
        reg.record("draw", ms(1));
        assert!(reg.summary().contains('3'));
    }

    #[test]
    fn empty_summary_says_so() {
        assert_eq!(Registry::new().summary(), "(no spans recorded)\n");
    }

    #[test]
    fn write_to_writes_the_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.txt");
        let reg = Registry::new();
        reg.record("scan", ms(7));
        reg.write_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), reg.summary());
    }

    #[test]
    fn durations_format_adaptively() {
        assert_eq!(format_duration(Duration::from_nanos(250)), "250ns");
        assert_eq!(format_duration(Duration::from_nanos(12_500)), "12.5µs");
        assert_eq!(format_duration(Duration::from_micros(3_400)), "3.40ms");
        assert_eq!(format_duration(Duration::from_millis(2_500)), "2.50s");
    }
}
