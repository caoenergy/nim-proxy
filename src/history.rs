//! Metrics history: every 5 minutes the sampler appends a full Prometheus
//! snapshot (the same text /metrics serves) to memory and, when a data dir is
//! writable, to history.jsonl. The dashboard's range queries replay these
//! snapshots through the same client-side parser it uses for live polls.
//! Snapshots are ~4 KB, so 30 days is ~35 MB — retention is a days knob
//! (HISTORY_DAYS, 0 = keep forever), not a size-management subsystem.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

pub const SAMPLE_SECS: u64 = 300;
const MAX_EXPOSITION_LINE_BYTES: usize = 1024 * 1024;
const MAX_EXPOSITION_SERIES: usize = 100_000;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct MetricKey {
    metric: String,
    labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetricValue {
    pub metric: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

#[derive(Default)]
struct ParsedSnapshot {
    counters: BTreeMap<MetricKey, f64>,
    gauges: BTreeMap<MetricKey, f64>,
    skipped_lines: usize,
}

#[derive(Clone, Copy)]
enum PrometheusType {
    Counter,
    Gauge,
    Histogram,
}

fn parse_exposition(exposition: &str) -> ParsedSnapshot {
    let mut parsed = ParsedSnapshot::default();
    let mut types = BTreeMap::new();
    let mut series = 0;

    for line in exposition.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(metadata) = line.strip_prefix("# TYPE ") {
            let mut fields = metadata.split_whitespace();
            let name = fields.next();
            let kind = fields.next();
            if fields.next().is_none() {
                let kind = match kind {
                    Some("counter") => Some(PrometheusType::Counter),
                    Some("gauge") => Some(PrometheusType::Gauge),
                    Some("histogram") => Some(PrometheusType::Histogram),
                    _ => None,
                };
                if let (Some(name), Some(kind)) = (name, kind) {
                    if valid_metric_name(name) {
                        types.insert(name.to_owned(), kind);
                    }
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.len() > MAX_EXPOSITION_LINE_BYTES || series >= MAX_EXPOSITION_SERIES {
            parsed.skipped_lines += 1;
            continue;
        }

        let split_at = line.rfind('}').map_or(0, |index| index + 1);
        let sample_and_value = if split_at == 0 {
            line.split_once(char::is_whitespace)
        } else {
            line[split_at..]
                .split_once(char::is_whitespace)
                .map(|(_, value)| (&line[..split_at], value))
        };
        let Some((series_text, value_text)) = sample_and_value else {
            parsed.skipped_lines += 1;
            continue;
        };
        let value_text = value_text.trim_start();
        if value_text.is_empty() || value_text.chars().any(char::is_whitespace) {
            parsed.skipped_lines += 1;
            continue;
        }
        let Ok(value) = value_text.parse::<f64>() else {
            parsed.skipped_lines += 1;
            continue;
        };
        if !value.is_finite() {
            parsed.skipped_lines += 1;
            continue;
        }
        let Some(key) = parse_metric_key(series_text) else {
            parsed.skipped_lines += 1;
            continue;
        };

        series += 1;
        if is_counter_like(&key.metric, &types) {
            parsed.counters.insert(key, value);
        } else {
            parsed.gauges.insert(key, value);
        }
    }

    parsed
}

fn valid_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_metric_key(series: &str) -> Option<MetricKey> {
    let Some(open) = series.find('{') else {
        return valid_metric_name(series).then(|| MetricKey {
            metric: series.to_owned(),
            labels: BTreeMap::new(),
        });
    };
    if !series.ends_with('}') {
        return None;
    }
    let metric = &series[..open];
    if !valid_metric_name(metric) {
        return None;
    }
    let labels = parse_labels(&series[open + 1..series.len() - 1])?;
    Some(MetricKey {
        metric: metric.to_owned(),
        labels,
    })
}

fn parse_labels(input: &str) -> Option<BTreeMap<String, String>> {
    let mut chars = input.chars().peekable();
    let mut labels = BTreeMap::new();

    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            return Some(labels);
        }

        let mut name = String::new();
        while let Some(c) = chars.next_if(|c| c.is_ascii_alphanumeric() || *c == '_') {
            name.push(c);
        }
        if !valid_label_name(&name) {
            return None;
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('=') {
            return None;
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('"') {
            return None;
        }

        let mut value = String::new();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => match chars.next()? {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    'n' => value.push('\n'),
                    _ => return None,
                },
                c => value.push(c),
            }
        }
        if labels.insert(name, value).is_some() {
            return None;
        }

        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.next() {
            Some(',') => {}
            None => return Some(labels),
            _ => return None,
        }
    }
}

fn is_counter_like(
    metric: &str,
    types: &BTreeMap<String, PrometheusType>,
) -> bool {
    if let Some(kind) = types.get(metric) {
        return matches!(
            kind,
            PrometheusType::Counter | PrometheusType::Histogram
        );
    }
    for suffix in ["_bucket", "_count", "_sum"] {
        if let Some(base) = metric.strip_suffix(suffix) {
            if matches!(types.get(base), Some(PrometheusType::Histogram)) {
                return true;
            }
        }
    }
    metric.ends_with("_total")
        || metric.ends_with("_bucket")
        || metric.ends_with("_count")
        || metric.ends_with("_sum")
}

#[derive(Default)]
struct NormalizedMetrics {
    deltas: BTreeMap<MetricKey, f64>,
    gauges: BTreeMap<MetricKey, f64>,
    inferred_reset: bool,
}

fn normalize(
    previous: Option<&ParsedSnapshot>,
    current: &ParsedSnapshot,
    reset: bool,
) -> NormalizedMetrics {
    let inferred_reset = !reset
        && previous.is_some_and(|previous| {
            current.counters.iter().any(|(key, current_value)| {
                previous.counters.get(key).is_some_and(|previous_value| {
                    current_value - previous_value < -f64::EPSILON
                })
            })
        });
    let reset_epoch = reset || inferred_reset;
    let mut deltas = BTreeMap::new();

    for (key, current_value) in &current.counters {
        let delta = if reset_epoch {
            *current_value
        } else {
            previous
                .and_then(|snapshot| snapshot.counters.get(key))
                .map_or(*current_value, |previous_value| {
                    let delta = current_value - previous_value;
                    if (-f64::EPSILON..0.0).contains(&delta) {
                        0.0
                    } else {
                        delta
                    }
                })
        };
        deltas.insert(key.clone(), delta);
    }

    NormalizedMetrics {
        deltas,
        gauges: current.gauges.clone(),
        inferred_reset,
    }
}

pub struct History {
    points: Mutex<Vec<(u64, String)>>,
    file: Option<PathBuf>,
    /// Retention in days (0 = keep forever). Atomic so the settings layer can
    /// retune it live; the sampler reads it on every append.
    days: AtomicU64,
    dropped_since_compact: Mutex<usize>,
}

impl History {
    pub fn load(dir: Option<PathBuf>, days: u64) -> Self {
        let mut points = Vec::new();
        let file = dir.and_then(|d| {
            if let Err(e) = fs::create_dir_all(&d) {
                tracing::warn!("history disabled: cannot create {}: {e}", d.display());
                return None;
            }
            let path = d.join("history.jsonl");
            if let Ok(f) = fs::File::open(&path) {
                for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let (Some(t), Some(m)) = (v["t"].as_u64(), v["m"].as_str()) {
                            points.push((t, m.to_owned()));
                        }
                    }
                }
            }
            // Verify writability up front so we warn once at boot, not on
            // every sample.
            match fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(_) => Some(path),
                Err(e) => {
                    tracing::warn!(
                        "history persistence disabled ({}: {e}); keeping in-memory only",
                        path.display()
                    );
                    None
                }
            }
        });
        points.sort_by_key(|p| p.0);
        tracing::info!(
            "history           {} snapshots loaded, retention {}",
            points.len(),
            if days == 0 {
                "infinite".to_owned()
            } else {
                format!("{days} days")
            }
        );
        Self {
            points: Mutex::new(points),
            file,
            days: AtomicU64::new(days),
            dropped_since_compact: Mutex::new(0),
        }
    }

    /// Retune retention live (settings-driven); applies on the next append.
    pub fn set_days(&self, days: u64) {
        self.days.store(days, Ordering::Relaxed);
    }

    pub fn append(&self, t: u64, snapshot: String) {
        let mut points = self.points.lock().unwrap();
        let days = self.days.load(Ordering::Relaxed);
        if days > 0 {
            let cutoff = t.saturating_sub(days * 86400);
            let before = points.len();
            points.retain(|p| p.0 >= cutoff);
            *self.dropped_since_compact.lock().unwrap() += before - points.len();
        }
        let line = serde_json::json!({"t": t, "m": snapshot}).to_string();
        points.push((t, snapshot));

        if let Some(path) = &self.file {
            let mut dropped = self.dropped_since_compact.lock().unwrap();
            // Compact once a day's worth of expired snapshots has built up;
            // otherwise just append.
            let result = if *dropped > 288 {
                *dropped = 0;
                let all = points
                    .iter()
                    .map(|(t, m)| serde_json::json!({"t": t, "m": m}).to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                fs::write(path, all + "\n")
            } else {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut f| writeln!(f, "{line}"))
            };
            if let Err(e) = result {
                tracing::warn!("history write failed: {e}");
            }
        }
    }

    /// Snapshots in [from, to], stride-sampled down to at most `max` plus the
    /// range's endpoints.
    pub fn range(&self, from: u64, to: u64, max: usize) -> Vec<(u64, String)> {
        let points = self.points.lock().unwrap();
        let hits: Vec<&(u64, String)> =
            points.iter().filter(|p| p.0 >= from && p.0 <= to).collect();
        let stride = hits.len().div_ceil(max.max(2));
        hits.iter()
            .enumerate()
            .filter(|(i, _)| i % stride == 0 || *i == hits.len() - 1)
            .map(|(_, p)| (*p).clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = r#"
# TYPE nimproxy_requests_total counter
nimproxy_requests_total{client="Mindmap",model="z-ai/glm-5.2",status="200"} 12
# TYPE nimproxy_active_requests gauge
nimproxy_active_requests 2
# TYPE nimproxy_ttft_seconds histogram
nimproxy_ttft_seconds_bucket{model="z-ai/glm-5.2",le="0.5"} 3
nimproxy_ttft_seconds_bucket{model="z-ai/glm-5.2",le="+Inf"} 4
nimproxy_ttft_seconds_sum{model="z-ai/glm-5.2"} 1.25
nimproxy_ttft_seconds_count{model="z-ai/glm-5.2"} 4
"#;

    #[test]
    fn parses_counter_gauge_and_histogram_series() {
        let parsed = parse_exposition(SNAPSHOT);
        assert_eq!(parsed.counters.len(), 5);
        assert_eq!(parsed.gauges.len(), 1);
        assert_eq!(parsed.skipped_lines, 0);
    }

    #[test]
    fn parses_escaped_label_values() {
        let series =
            r#"escaped_total{quote="say \"hi\"",path="c:\\tmp",line="a\nb"}"#;
        let parsed =
            parse_exposition(&format!("# TYPE escaped_total counter\n{series} 1\n"));
        assert_eq!(parsed.skipped_lines, 0);
        let key = parsed.counters.keys().next().unwrap();
        assert_eq!(key.labels["quote"], "say \"hi\"");
        assert_eq!(key.labels["path"], "c:\\tmp");
        assert_eq!(key.labels["line"], "a\nb");
    }

    #[test]
    fn parse_skips_nan_and_counts_malformed_lines() {
        let parsed = parse_exposition(
            "# TYPE a_total counter\n\
             a_total NaN\n\
             missing_value\n\
             bad-name_total 2\n",
        );
        assert!(parsed.counters.is_empty());
        assert_eq!(parsed.skipped_lines, 3);
    }

    #[test]
    fn parses_counter_suffix_without_type_metadata() {
        let parsed = parse_exposition("orphan_total{kind=\"legacy\"} 9\n");
        let (key, value) = parsed.counters.iter().next().unwrap();
        assert_eq!(key.metric, "orphan_total");
        assert_eq!(key.labels["kind"], "legacy");
        assert_eq!(*value, 9.0);
        assert!(parsed.gauges.is_empty());
        assert_eq!(parsed.skipped_lines, 0);
    }

    fn metric(values: &BTreeMap<MetricKey, f64>, name: &str) -> f64 {
        values
            .iter()
            .find_map(|(key, value)| (key.metric == name).then_some(*value))
            .unwrap()
    }

    #[test]
    fn explicit_boot_change_counts_new_process_values() {
        let first =
            parse_exposition("# TYPE requests_total counter\nrequests_total 100\n");
        let second =
            parse_exposition("# TYPE requests_total counter\nrequests_total 7\n");
        let normalized = normalize(Some(&first), &second, true);
        assert_eq!(metric(&normalized.deltas, "requests_total"), 7.0);
    }

    #[test]
    fn one_legacy_counter_decrease_resets_the_snapshot_epoch() {
        let first = parse_exposition(
            "# TYPE a_total counter\na_total 100\n\
             # TYPE b_total counter\nb_total 20\n",
        );
        let second = parse_exposition(
            "# TYPE a_total counter\na_total 3\n\
             # TYPE b_total counter\nb_total 25\n",
        );
        let normalized = normalize(Some(&first), &second, false);
        assert!(normalized.inferred_reset);
        assert_eq!(metric(&normalized.deltas, "a_total"), 3.0);
        assert_eq!(metric(&normalized.deltas, "b_total"), 25.0);
    }

    #[test]
    fn normalize_subtracts_counters_without_a_reset() {
        let first =
            parse_exposition("# TYPE requests_total counter\nrequests_total 100\n");
        let second =
            parse_exposition("# TYPE requests_total counter\nrequests_total 107\n");
        let normalized = normalize(Some(&first), &second, false);
        assert!(!normalized.inferred_reset);
        assert_eq!(metric(&normalized.deltas, "requests_total"), 7.0);
    }

    #[test]
    fn normalize_counts_a_new_counter_from_zero() {
        let first = parse_exposition("# TYPE old_total counter\nold_total 4\n");
        let second = parse_exposition(
            "# TYPE old_total counter\nold_total 6\n\
             # TYPE new_total counter\nnew_total 9\n",
        );
        let normalized = normalize(Some(&first), &second, false);
        assert_eq!(metric(&normalized.deltas, "old_total"), 2.0);
        assert_eq!(metric(&normalized.deltas, "new_total"), 9.0);
    }

    #[test]
    fn normalize_copies_current_gauges() {
        let first = parse_exposition(
            "# TYPE active gauge\nactive 8\n\
             # TYPE requests_total counter\nrequests_total 3\n",
        );
        let second = parse_exposition(
            "# TYPE active gauge\nactive 2\n\
             # TYPE requests_total counter\nrequests_total 4\n",
        );
        let normalized = normalize(Some(&first), &second, false);
        assert_eq!(metric(&normalized.gauges, "active"), 2.0);
        assert_eq!(metric(&normalized.deltas, "requests_total"), 1.0);
    }

    #[test]
    fn retention_prunes_and_range_filters() {
        let h = History {
            points: Mutex::new(Vec::new()),
            file: None,
            days: AtomicU64::new(1),
            dropped_since_compact: Mutex::new(0),
        };
        h.append(1_000, "old".into());
        h.append(200_000, "new".into()); // 1-day cutoff drops t=1000
        assert_eq!(h.range(0, u64::MAX, 100).len(), 1);
        h.append(200_300, "newer".into());
        let r = h.range(200_000, 200_100, 100);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "new");
    }

    #[test]
    fn range_downsamples_to_max() {
        let h = History {
            points: Mutex::new((0..1000u64).map(|i| (i, i.to_string())).collect()),
            file: None,
            days: AtomicU64::new(0),
            dropped_since_compact: Mutex::new(0),
        };
        let r = h.range(0, 999, 100);
        assert!(r.len() <= 101, "got {}", r.len());
        assert_eq!(r.last().unwrap().0, 999, "endpoint kept");
    }

    /// A unique per-test scratch dir (std-only; removed on drop).
    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "nimproxy-history-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_reads_existing_snapshots_and_skips_junk() {
        let dir = TestDir::new();
        let path = dir.0.join("history.jsonl");
        // Two valid lines, one unparseable line (skipped), one out-of-order.
        fs::write(
            &path,
            "{\"t\":10,\"m\":\"a\"}\nnot json\n{\"t\":20,\"m\":\"b\"}\n{\"t\":5,\"m\":\"c\"}\n",
        )
        .unwrap();
        // days = 0 exercises the "infinite" retention log branch.
        let h = History::load(Some(dir.0.clone()), 0);
        let all = h.range(0, u64::MAX, 100);
        assert_eq!(all.len(), 3, "3 valid lines parsed, junk skipped");
        assert_eq!(all[0].0, 5, "snapshots sorted by timestamp on load");
        // days > 0 exercises the "{days} days" retention log branch.
        let h2 = History::load(Some(dir.0.clone()), 7);
        assert_eq!(h2.range(0, u64::MAX, 100).len(), 3);
    }

    #[test]
    fn append_compacts_the_file_after_a_days_worth_of_expiry() {
        let dir = TestDir::new();
        let path = dir.0.join("history.jsonl");
        fs::write(&path, "{\"t\":1,\"m\":\"old\"}\n").unwrap();
        let h = History {
            points: Mutex::new(vec![(1, "old".into())]),
            file: Some(path.clone()),
            days: AtomicU64::new(1),
            // One more expiry crosses the >288 compaction threshold.
            dropped_since_compact: Mutex::new(288),
        };
        // days = 1: cutoff = 200_000 - 86_400 = 113_600, so the t=1 snapshot
        // expires; that pushes the drop count to 289 (>288) and triggers a full
        // file rewrite rather than an append.
        h.append(200_000, "new".into());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("new"), "surviving snapshot rewritten");
        assert!(
            !contents.contains("old"),
            "expired snapshot compacted out of the file"
        );
    }

    #[test]
    fn load_disables_persistence_when_the_dir_cant_be_created() {
        let dir = TestDir::new();
        // A file sits where the history directory should be, so create_dir_all
        // fails and persistence falls back to in-memory only.
        let blocker = dir.0.join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let h = History::load(Some(blocker.join("sub")), 1);
        assert!(
            h.file.is_none(),
            "persistence disabled on dir-create failure"
        );
    }

    #[test]
    fn load_disables_persistence_when_the_path_isnt_a_writable_file() {
        let dir = TestDir::new();
        // A directory sits where history.jsonl should be, so opening it for
        // append fails (EISDIR) and persistence stays in-memory.
        fs::create_dir_all(dir.0.join("history.jsonl")).unwrap();
        let h = History::load(Some(dir.0.clone()), 1);
        assert!(
            h.file.is_none(),
            "persistence disabled when the path isn't a file"
        );
    }

    #[test]
    fn append_survives_a_write_failure_without_panicking() {
        let dir = TestDir::new();
        // `file` points at a directory: the append write errors and is logged,
        // but the in-memory record still updates and nothing panics.
        let h = History {
            points: Mutex::new(Vec::new()),
            file: Some(dir.0.clone()),
            days: AtomicU64::new(0),
            dropped_since_compact: Mutex::new(0),
        };
        h.append(1, "snap".into());
        assert_eq!(
            h.range(0, u64::MAX, 10).len(),
            1,
            "in-memory append still works"
        );
    }
}
