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

use serde::{Deserialize, Serialize};

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
        if line.len() > MAX_EXPOSITION_LINE_BYTES {
            parsed.skipped_lines += 1;
            continue;
        }
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
                        if types.len() < MAX_EXPOSITION_SERIES || types.contains_key(name) {
                            types.insert(name.to_owned(), kind);
                        } else {
                            parsed.skipped_lines += 1;
                        }
                    }
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if series >= MAX_EXPOSITION_SERIES {
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

fn is_counter_like(metric: &str, types: &BTreeMap<String, PrometheusType>) -> bool {
    if let Some(kind) = types.get(metric) {
        return matches!(kind, PrometheusType::Counter | PrometheusType::Histogram);
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
                previous
                    .counters
                    .get(key)
                    .is_some_and(|previous_value| current_value - previous_value < -f64::EPSILON)
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CapacitySnapshot {
    pub enabled_lanes: usize,
    pub rpms: Vec<usize>,
    pub capacity_rpm: usize,
}

struct IndexedPoint {
    t: u64,
    deltas: BTreeMap<MetricKey, f64>,
    gauges: BTreeMap<MetricKey, f64>,
    capacity: Option<CapacitySnapshot>,
}

#[derive(Default)]
struct HistoryInner {
    points: Vec<IndexedPoint>,
    revision: u64,
    available_from: Option<u64>,
    available_to: Option<u64>,
    diagnostics: HistoryDiagnostics,
    last_parsed: Option<ParsedSnapshot>,
    last_sample_boot: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HistoryDiagnostics {
    pub valid_samples: usize,
    pub skipped_records: usize,
    pub skipped_metric_lines: usize,
    pub normalized_series: usize,
    pub legacy_resets_inferred: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryStatus {
    pub available_from: Option<u64>,
    pub available_to: Option<u64>,
    pub file_bytes: u64,
    pub compaction_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CapacityRollup {
    pub average_rpm: f64,
    pub latest_rpms: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RollupPoint {
    pub from: u64,
    pub to: u64,
    pub duration_seconds: u64,
    pub values: Vec<MetricValue>,
    pub capacity: Option<CapacityRollup>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rollup {
    pub available_from: Option<u64>,
    pub available_to: Option<u64>,
    pub effective_from: Option<u64>,
    pub effective_to: Option<u64>,
    pub totals: Vec<MetricValue>,
    pub latest: Vec<MetricValue>,
    pub points: Vec<RollupPoint>,
    pub diagnostics: HistoryDiagnostics,
    pub history_revision: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tail {
    pub base_history_revision: u64,
    pub from: Option<u64>,
    pub to: u64,
    pub totals: Vec<MetricValue>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CurrentMetrics {
    pub metrics: Vec<MetricValue>,
    pub tail: Tail,
}

#[derive(Clone)]
struct RawPoint {
    t: u64,
    m: String,
    boot: Option<String>,
    capacity: Option<CapacitySnapshot>,
}

#[derive(Deserialize)]
struct StoredRecord {
    v: Option<u8>,
    t: Option<u64>,
    boot: Option<String>,
    kind: Option<String>,
    capacity: Option<CapacitySnapshot>,
    m: Option<String>,
}

enum LoadedRecord {
    Boot { t: u64 },
    Sample(RawPoint),
}

impl LoadedRecord {
    fn timestamp(&self) -> u64 {
        match self {
            Self::Boot { t, .. } => *t,
            Self::Sample(point) => point.t,
        }
    }
}

#[derive(Serialize)]
struct BootRecord<'a> {
    v: u8,
    t: u64,
    boot: &'a str,
    kind: &'static str,
    capacity: &'a CapacitySnapshot,
}

#[derive(Serialize)]
struct SampleRecord<'a> {
    v: u8,
    t: u64,
    boot: &'a str,
    capacity: &'a CapacitySnapshot,
    m: &'a str,
}

#[derive(Serialize)]
struct LegacySampleRecord<'a> {
    t: u64,
    m: &'a str,
}

pub struct History {
    /// Temporary raw compatibility storage for `/api/history`; Task 7 removes
    /// it after the route switches to typed rollups.
    points: Mutex<Vec<RawPoint>>,
    inner: Mutex<HistoryInner>,
    file: Option<PathBuf>,
    /// Retention in days (0 = keep forever). Atomic so the settings layer can
    /// retune it live; the sampler reads it on every append.
    days: AtomicU64,
    dropped_since_compact: Mutex<usize>,
    boot_id: String,
    boot_t: u64,
    initial_capacity: CapacitySnapshot,
}

impl History {
    pub fn load(dir: Option<PathBuf>, days: u64, initial_capacity: CapacitySnapshot) -> Self {
        let started = std::time::Instant::now();
        let mut source_bytes = 0;
        let mut diagnostics = HistoryDiagnostics::default();
        let mut records = Vec::new();

        let file = dir.and_then(|d| {
            if let Err(e) = fs::create_dir_all(&d) {
                tracing::warn!("history disabled: cannot create {}: {e}", d.display());
                return None;
            }
            let path = d.join("history.jsonl");
            if let Ok(f) = fs::File::open(&path) {
                source_bytes = f.metadata().map_or(0, |metadata| metadata.len());
                for line in std::io::BufReader::new(f).lines() {
                    let Ok(line) = line else {
                        diagnostics.skipped_records += 1;
                        break;
                    };
                    let Ok(record) = serde_json::from_str::<StoredRecord>(&line) else {
                        diagnostics.skipped_records += 1;
                        continue;
                    };
                    match decode_record(record) {
                        Some(record) => records.push(record),
                        None => diagnostics.skipped_records += 1,
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

        records.sort_by_key(LoadedRecord::timestamp);
        let mut raw_points = Vec::new();
        let mut indexed_points = Vec::new();
        let mut last_parsed: Option<ParsedSnapshot> = None;
        let mut last_sample_boot: Option<String> = None;
        for record in records {
            let LoadedRecord::Sample(point) = record else {
                continue;
            };
            let current = parse_exposition(&point.m);
            let explicit_reset = last_parsed.is_some() && point.boot != last_sample_boot;
            let legacy_empty_reset = !explicit_reset
                && point.boot.is_none()
                && last_sample_boot.is_none()
                && last_parsed
                    .as_ref()
                    .is_some_and(|previous| previous.counters.is_empty())
                && !current.counters.is_empty();
            let mut normalized = normalize(
                last_parsed.as_ref(),
                &current,
                explicit_reset || legacy_empty_reset,
            );
            if legacy_empty_reset {
                normalized.inferred_reset = true;
            }
            diagnostics.valid_samples += 1;
            diagnostics.skipped_metric_lines += current.skipped_lines;
            diagnostics.normalized_series += normalized.deltas.len() + normalized.gauges.len();
            if normalized.inferred_reset {
                diagnostics.legacy_resets_inferred += 1;
            }
            indexed_points.push(IndexedPoint {
                t: point.t,
                deltas: normalized.deltas,
                gauges: normalized.gauges,
                capacity: point.capacity.clone(),
            });
            last_parsed = Some(current);
            last_sample_boot = point.boot.clone();
            raw_points.push(point);
        }

        let available_from = indexed_points.first().map(|point| point.t);
        let available_to = indexed_points.last().map(|point| point.t);
        let revision = diagnostics.valid_samples as u64;
        let boot_id = new_boot_id();
        let boot_t = unix_now();
        let history = Self {
            points: Mutex::new(raw_points),
            inner: Mutex::new(HistoryInner {
                points: indexed_points,
                revision,
                available_from,
                available_to,
                diagnostics,
                last_parsed,
                last_sample_boot,
            }),
            file,
            days: AtomicU64::new(days),
            dropped_since_compact: Mutex::new(0),
            boot_id,
            boot_t,
            initial_capacity,
        };
        history.persist_boot_marker();
        let inner = history.inner.lock().unwrap();
        tracing::info!(
            "history           {} bytes, {} samples, {} skipped records, {} skipped metric lines, \
             {} normalized series, {} inferred resets, indexed in {:?}; retention {}",
            source_bytes,
            inner.diagnostics.valid_samples,
            inner.diagnostics.skipped_records,
            inner.diagnostics.skipped_metric_lines,
            inner.diagnostics.normalized_series,
            inner.diagnostics.legacy_resets_inferred,
            started.elapsed(),
            if days == 0 {
                "infinite".to_owned()
            } else {
                format!("{days} days")
            }
        );
        drop(inner);
        history
    }

    /// Retune retention live (settings-driven); applies on the next append.
    pub fn set_days(&self, days: u64) {
        self.days.store(days, Ordering::Relaxed);
    }

    pub fn revision(&self) -> u64 {
        self.inner.lock().unwrap().revision
    }

    pub fn status(&self) -> HistoryStatus {
        let (available_from, available_to) = {
            let inner = self.inner.lock().unwrap();
            (inner.available_from, inner.available_to)
        };
        let compaction_pending = *self.dropped_since_compact.lock().unwrap() > 0;
        let file_bytes = self
            .file
            .as_ref()
            .and_then(|path| fs::metadata(path).ok())
            .map_or(0, |metadata| metadata.len());
        HistoryStatus {
            available_from,
            available_to,
            file_bytes,
            compaction_pending,
        }
    }

    pub fn append(&self, t: u64, snapshot: &str, capacity: CapacitySnapshot) {
        let current = parse_exposition(snapshot);
        let mut inner = self.inner.lock().unwrap();
        // Wall clocks can move backward, and a retained file can contain a
        // future-dated point. Keep ingestion order without fabricating time:
        // equal timestamps preserve binary-search ordering and exact deltas.
        let t = inner.available_to.map_or(t, |latest| latest.max(t));
        let explicit_reset = inner.last_parsed.is_some()
            && inner.last_sample_boot.as_deref() != Some(self.boot_id.as_str());
        let normalized = normalize(inner.last_parsed.as_ref(), &current, explicit_reset);
        inner.diagnostics.valid_samples += 1;
        inner.diagnostics.skipped_metric_lines += current.skipped_lines;
        inner.diagnostics.normalized_series += normalized.deltas.len() + normalized.gauges.len();
        if normalized.inferred_reset {
            inner.diagnostics.legacy_resets_inferred += 1;
        }

        let days = self.days.load(Ordering::Relaxed);
        let mut raw_points = self.points.lock().unwrap();
        if days > 0 {
            let cutoff = t.saturating_sub(days * 86400);
            let before = inner.points.len();
            inner.points.retain(|point| point.t >= cutoff);
            raw_points.retain(|point| point.t >= cutoff);
            *self.dropped_since_compact.lock().unwrap() += before - inner.points.len();
        }
        inner.points.push(IndexedPoint {
            t,
            deltas: normalized.deltas,
            gauges: normalized.gauges,
            capacity: Some(capacity.clone()),
        });
        raw_points.push(RawPoint {
            t,
            m: snapshot.to_owned(),
            boot: Some(self.boot_id.clone()),
            capacity: Some(capacity.clone()),
        });
        inner.revision = inner.revision.wrapping_add(1);
        inner.available_from = inner.points.first().map(|point| point.t);
        inner.available_to = inner.points.last().map(|point| point.t);
        inner.last_parsed = Some(current);
        inner.last_sample_boot = Some(self.boot_id.clone());
        drop(inner);

        let line = serde_json::to_string(&SampleRecord {
            v: 2,
            t,
            boot: &self.boot_id,
            capacity: &capacity,
            m: snapshot,
        })
        .expect("history sample record serializes");
        self.persist_sample(line, &raw_points);
    }

    pub fn rollup(&self, from: u64, to: u64, points: usize) -> Rollup {
        let inner = self.inner.lock().unwrap();
        let empty = || Rollup {
            available_from: inner.available_from,
            available_to: inner.available_to,
            effective_from: None,
            effective_to: None,
            totals: Vec::new(),
            latest: Vec::new(),
            points: Vec::new(),
            diagnostics: inner.diagnostics.clone(),
            history_revision: inner.revision,
        };
        if from >= to {
            return empty();
        }

        let first = inner.points.partition_point(|point| point.t <= from);
        let end = inner.points.partition_point(|point| point.t <= to);
        if first == end {
            return empty();
        }

        let point_budget = points.clamp(2, 1000);
        let mut totals = BTreeMap::new();
        let mut latest = BTreeMap::new();
        let mut buckets: Vec<BucketAccumulator> = (0..point_budget)
            .map(|_| BucketAccumulator::default())
            .collect();

        for index in first..end {
            let point = &inner.points[index];
            add_metrics(&mut totals, &point.deltas);
            latest.extend(point.gauges.clone());

            let offset = point.t.saturating_sub(from) as u128;
            let width = to.saturating_sub(from) as u128;
            let bucket_index = ((offset * point_budget as u128) / width) as usize;
            let bucket_index = bucket_index.min(point_budget - 1);
            let bucket = &mut buckets[bucket_index];
            add_metrics(&mut bucket.deltas, &point.deltas);
            bucket.gauges.extend(point.gauges.clone());

            let interval_start = if index == 0 {
                from
            } else {
                inner.points[index - 1].t.max(from)
            };
            let interval_end = point.t.min(to);
            let duration = interval_end.saturating_sub(interval_start);
            bucket.from = Some(
                bucket
                    .from
                    .map_or(interval_start, |current| current.min(interval_start)),
            );
            bucket.to = Some(
                bucket
                    .to
                    .map_or(interval_end, |current| current.max(interval_end)),
            );
            bucket.duration_seconds += duration;
            bucket.capacity_duration += duration;
            if let Some(capacity) = &point.capacity {
                bucket.capacity_weight += capacity.capacity_rpm as f64 * duration as f64;
                bucket.known_capacity_duration += duration;
                bucket.latest_rpms = capacity.rpms.clone();
            }
        }

        let effective_from = if first == 0 {
            Some(inner.points[first].t)
        } else {
            Some(from.max(inner.points[first - 1].t))
        };
        let effective_to = Some(inner.points[end - 1].t);
        let points = buckets
            .into_iter()
            .filter_map(BucketAccumulator::finish)
            .collect();

        Rollup {
            available_from: inner.available_from,
            available_to: inner.available_to,
            effective_from,
            effective_to,
            totals: metric_values(totals),
            latest: metric_values(latest),
            points,
            diagnostics: inner.diagnostics.clone(),
            history_revision: inner.revision,
        }
    }

    pub fn current(&self, t: u64, current_exposition: &str) -> CurrentMetrics {
        let current = parse_exposition(current_exposition);
        let inner = self.inner.lock().unwrap();
        let reset = inner.last_parsed.is_some()
            && inner.last_sample_boot.as_deref() != Some(self.boot_id.as_str());
        let normalized = normalize(inner.last_parsed.as_ref(), &current, reset);
        let mut metrics = current.counters.clone();
        metrics.extend(current.gauges);

        CurrentMetrics {
            metrics: metric_values(metrics),
            tail: Tail {
                base_history_revision: inner.revision,
                from: inner.available_to,
                to: t,
                totals: metric_values(normalized.deltas),
            },
        }
    }

    /// Snapshots in [from, to], stride-sampled down to at most `max` plus the
    /// range's endpoints. Temporary raw compatibility until Task 7.
    pub fn range(&self, from: u64, to: u64, max: usize) -> Vec<(u64, String)> {
        let points = self.points.lock().unwrap();
        let hits: Vec<&RawPoint> = points
            .iter()
            .filter(|point| point.t >= from && point.t <= to)
            .collect();
        let stride = hits.len().div_ceil(max.max(2));
        hits.iter()
            .enumerate()
            .filter(|(index, _)| index % stride == 0 || *index == hits.len() - 1)
            .map(|(_, point)| (point.t, point.m.clone()))
            .collect()
    }

    fn persist_boot_marker(&self) {
        let Some(path) = &self.file else {
            return;
        };
        let line = serde_json::to_string(&BootRecord {
            v: 2,
            t: self.boot_t,
            boot: &self.boot_id,
            kind: "boot",
            capacity: &self.initial_capacity,
        })
        .expect("history boot record serializes");
        if let Err(e) = append_line(path, &line) {
            tracing::warn!("history boot marker write failed: {e}");
        }
    }

    fn persist_sample(&self, line: String, points: &[RawPoint]) {
        let Some(path) = &self.file else {
            return;
        };
        let mut dropped = self.dropped_since_compact.lock().unwrap();
        // Compact once a day's worth of expired snapshots has built up;
        // otherwise just append.
        let result = if *dropped > 288 {
            *dropped = 0;
            let mut lines = vec![serde_json::to_string(&BootRecord {
                v: 2,
                t: self.boot_t,
                boot: &self.boot_id,
                kind: "boot",
                capacity: &self.initial_capacity,
            })
            .expect("history boot record serializes")];
            lines.extend(points.iter().map(serialize_raw_point));
            fs::write(path, lines.join("\n") + "\n")
        } else {
            append_line(path, &line)
        };
        if let Err(e) = result {
            tracing::warn!("history write failed: {e}");
        }
    }
}

#[derive(Default)]
struct BucketAccumulator {
    deltas: BTreeMap<MetricKey, f64>,
    gauges: BTreeMap<MetricKey, f64>,
    from: Option<u64>,
    to: Option<u64>,
    duration_seconds: u64,
    capacity_weight: f64,
    capacity_duration: u64,
    known_capacity_duration: u64,
    latest_rpms: Vec<usize>,
}

impl BucketAccumulator {
    fn finish(self) -> Option<RollupPoint> {
        let from = self.from?;
        let to = self.to?;
        let capacity = (self.capacity_duration > 0
            && self.known_capacity_duration == self.capacity_duration)
            .then(|| CapacityRollup {
                average_rpm: self.capacity_weight / self.capacity_duration as f64,
                latest_rpms: self.latest_rpms,
            });
        let mut values = self.deltas;
        values.extend(self.gauges);
        Some(RollupPoint {
            from,
            to,
            duration_seconds: self.duration_seconds,
            values: metric_values(values),
            capacity,
        })
    }
}

fn add_metrics(totals: &mut BTreeMap<MetricKey, f64>, values: &BTreeMap<MetricKey, f64>) {
    for (key, value) in values {
        *totals.entry(key.clone()).or_default() += value;
    }
}

fn metric_values(values: BTreeMap<MetricKey, f64>) -> Vec<MetricValue> {
    values
        .into_iter()
        .map(|(key, value)| MetricValue {
            metric: key.metric,
            labels: key.labels,
            value,
        })
        .collect()
}

fn decode_record(record: StoredRecord) -> Option<LoadedRecord> {
    let t = record.t?;
    match (record.v, record.kind.as_deref()) {
        (Some(2), Some("boot")) => {
            record.boot?;
            record.capacity?;
            Some(LoadedRecord::Boot { t })
        }
        (Some(2), None) => Some(LoadedRecord::Sample(RawPoint {
            t,
            m: record.m?,
            boot: Some(record.boot?),
            capacity: Some(record.capacity?),
        })),
        (None, None) => Some(LoadedRecord::Sample(RawPoint {
            t,
            m: record.m?,
            boot: None,
            capacity: None,
        })),
        _ => None,
    }
}

fn new_boot_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS RNG for history boot ID");
    let mut id = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn append_line(path: &PathBuf, line: &str) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"))
}

fn serialize_raw_point(point: &RawPoint) -> String {
    match (&point.boot, &point.capacity) {
        (Some(boot), Some(capacity)) => serde_json::to_string(&SampleRecord {
            v: 2,
            t: point.t,
            boot,
            capacity,
            m: &point.m,
        }),
        _ => serde_json::to_string(&LegacySampleRecord {
            t: point.t,
            m: &point.m,
        }),
    }
    .expect("history raw point serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn capacity(rpm: usize) -> CapacitySnapshot {
        CapacitySnapshot {
            enabled_lanes: usize::from(rpm > 0),
            rpms: (rpm > 0).then_some(rpm).into_iter().collect(),
            capacity_rpm: rpm,
        }
    }

    fn memory_history(days: u64) -> History {
        History::load(None, days, capacity(40))
    }

    fn history_with_file(days: u64, file: PathBuf) -> History {
        History {
            points: Mutex::new(Vec::new()),
            inner: Mutex::new(HistoryInner::default()),
            file: Some(file),
            days: AtomicU64::new(days),
            dropped_since_compact: Mutex::new(0),
            boot_id: "00000000000000000000000000000000".to_owned(),
            boot_t: 0,
            initial_capacity: capacity(40),
        }
    }

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
        let series = r#"escaped_total{quote="say \"hi\"",path="c:\\tmp",line="a\nb"}"#;
        let parsed = parse_exposition(&format!("# TYPE escaped_total counter\n{series} 1\n"));
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

    #[test]
    fn parse_counts_oversized_comments_as_skipped() {
        let exposition = format!(
            "#{}\nrequests_total 1\n",
            "x".repeat(MAX_EXPOSITION_LINE_BYTES)
        );
        let parsed = parse_exposition(&exposition);
        assert_eq!(parsed.skipped_lines, 1);
        assert_eq!(metric(&parsed.counters, "requests_total"), 1.0);
    }

    #[test]
    fn parse_rejects_oversized_type_metadata_before_classification() {
        let exposition = format!(
            "# TYPE forced_total{}gauge\nforced_total 1\n",
            " ".repeat(MAX_EXPOSITION_LINE_BYTES)
        );
        let parsed = parse_exposition(&exposition);
        assert_eq!(parsed.skipped_lines, 1);
        assert_eq!(metric(&parsed.counters, "forced_total"), 1.0);
        assert!(parsed.gauges.is_empty());
    }

    #[test]
    fn parse_caps_the_number_of_retained_series() {
        let mut exposition = String::new();
        for value in 0..=MAX_EXPOSITION_SERIES {
            writeln!(&mut exposition, "capped_total {value}").unwrap();
        }
        let parsed = parse_exposition(&exposition);
        assert_eq!(parsed.skipped_lines, 1);
        assert_eq!(
            metric(&parsed.counters, "capped_total"),
            (MAX_EXPOSITION_SERIES - 1) as f64
        );
    }

    #[test]
    fn parse_caps_retained_type_metadata() {
        let mut exposition = String::new();
        for value in 0..MAX_EXPOSITION_SERIES {
            writeln!(&mut exposition, "# TYPE metric_{value} gauge").unwrap();
        }
        exposition.push_str("# TYPE overflow_total gauge\noverflow_total 1\n");
        let parsed = parse_exposition(&exposition);
        assert_eq!(parsed.skipped_lines, 1);
        assert_eq!(metric(&parsed.counters, "overflow_total"), 1.0);
        assert!(parsed.gauges.is_empty());
    }

    fn metric(values: &BTreeMap<MetricKey, f64>, name: &str) -> f64 {
        values
            .iter()
            .find_map(|(key, value)| (key.metric == name).then_some(*value))
            .unwrap()
    }

    #[test]
    fn explicit_boot_change_counts_new_process_values() {
        let first = parse_exposition("# TYPE requests_total counter\nrequests_total 100\n");
        let second = parse_exposition("# TYPE requests_total counter\nrequests_total 7\n");
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
        let first = parse_exposition("# TYPE requests_total counter\nrequests_total 100\n");
        let second = parse_exposition("# TYPE requests_total counter\nrequests_total 107\n");
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
        let h = memory_history(1);
        h.append(1_000, "old", capacity(40));
        h.append(200_000, "new", capacity(40)); // 1-day cutoff drops t=1000
        assert_eq!(h.range(0, u64::MAX, 100).len(), 1);
        h.append(200_300, "newer", capacity(40));
        let r = h.range(200_000, 200_100, 100);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "new");
    }

    #[test]
    fn append_clamps_clock_rollback_to_preserve_sorted_index() {
        let history = memory_history(0);
        history.append(
            300,
            "# TYPE requests_total counter\nrequests_total 10\n",
            capacity(40),
        );
        history.append(
            200,
            "# TYPE requests_total counter\nrequests_total 15\n",
            capacity(40),
        );

        let timestamps = history
            .inner
            .lock()
            .unwrap()
            .points
            .iter()
            .map(|point| point.t)
            .collect::<Vec<_>>();
        assert_eq!(timestamps, [300, 300]);
        assert!(timestamps.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(history.status().available_to, Some(300));
        let rollup = history.rollup(299, 300, 20);
        assert_eq!(value(&rollup.totals, "requests_total"), 15.0);
    }

    #[test]
    fn range_downsamples_to_max() {
        let h = memory_history(0);
        for i in 0..1000u64 {
            h.append(i, &i.to_string(), capacity(40));
        }
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
    fn load_reads_v1_and_v2_boot_epochs() {
        let dir = TestDir::new();
        let path = dir.0.join("history.jsonl");
        fs::write(
            &path,
            "{\"t\":10,\"m\":\"# TYPE requests_total counter\\nrequests_total 5\\n\"}\n\
             {\"t\":20,\"m\":\"# TYPE requests_total counter\\nrequests_total 8\\n\"}\n\
             {\"v\":2,\"t\":21,\"boot\":\"boot-b\",\"kind\":\"boot\",\"capacity\":{\"enabled_lanes\":2,\"rpms\":[40,30],\"capacity_rpm\":70}}\n\
             {\"v\":2,\"t\":30,\"boot\":\"boot-b\",\"capacity\":{\"enabled_lanes\":2,\"rpms\":[40,30],\"capacity_rpm\":70},\"m\":\"# TYPE requests_total counter\\nrequests_total 2\\n\"}\n",
        )
        .unwrap();

        let h = History::load(
            Some(dir.0.clone()),
            0,
            CapacitySnapshot {
                enabled_lanes: 1,
                rpms: vec![40],
                capacity_rpm: 40,
            },
        );
        let inner = h.inner.lock().unwrap();
        assert_eq!(inner.points.len(), 3, "boot marker is not a metric sample");
        assert_eq!(
            inner
                .points
                .iter()
                .map(|point| metric(&point.deltas, "requests_total"))
                .sum::<f64>(),
            10.0
        );
        assert_eq!(inner.points[2].capacity.as_ref().unwrap().rpms, [40, 30]);
        assert_eq!(inner.diagnostics.legacy_resets_inferred, 0);
    }

    #[test]
    fn load_infers_reset_when_legacy_counters_empty_then_reappear() {
        let dir = TestDir::new();
        let path = dir.0.join("history.jsonl");
        fs::write(
            &path,
            "{\"t\":10,\"m\":\"# TYPE requests_total counter\\nrequests_total 5\\n\"}\n\
             {\"t\":20,\"m\":\"# TYPE active gauge\\nactive 1\\n\"}\n\
             {\"t\":30,\"m\":\"# TYPE requests_total counter\\nrequests_total 2\\n\"}\n",
        )
        .unwrap();

        let history = History::load(Some(dir.0.clone()), 0, capacity(40));
        let inner = history.inner.lock().unwrap();
        let total = inner
            .points
            .iter()
            .filter_map(|point| {
                point
                    .deltas
                    .iter()
                    .find_map(|(key, value)| (key.metric == "requests_total").then_some(*value))
            })
            .sum::<f64>();
        assert_eq!(total, 7.0);
        assert_eq!(inner.diagnostics.legacy_resets_inferred, 1);
    }

    fn history_with_points() -> History {
        let history = memory_history(0);
        history.append(
            100,
            "# TYPE requests_total counter\n\
             requests_total 10\n\
             # TYPE prompt_tokens_total counter\n\
             prompt_tokens_total{model=\"alpha\"} 100\n\
             prompt_tokens_total{model=\"beta\"} 50\n\
             # TYPE latency_seconds histogram\n\
             latency_seconds_bucket{le=\"0.5\"} 3\n\
             latency_seconds_bucket{le=\"+Inf\"} 10\n\
             latency_seconds_sum 4\n\
             latency_seconds_count 10\n\
             # TYPE active gauge\n\
             active 1\n",
            capacity(40),
        );
        history.append(
            200,
            "# TYPE requests_total counter\n\
             requests_total 25\n\
             # TYPE prompt_tokens_total counter\n\
             prompt_tokens_total{model=\"alpha\"} 150\n\
             prompt_tokens_total{model=\"beta\"} 80\n\
             # TYPE latency_seconds histogram\n\
             latency_seconds_bucket{le=\"0.5\"} 8\n\
             latency_seconds_bucket{le=\"+Inf\"} 25\n\
             latency_seconds_sum 11\n\
             latency_seconds_count 25\n\
             # TYPE active gauge\n\
             active 2\n",
            CapacitySnapshot {
                enabled_lanes: 2,
                rpms: vec![40, 40],
                capacity_rpm: 80,
            },
        );
        history.append(
            300,
            "# TYPE requests_total counter\n\
             requests_total 40\n\
             # TYPE prompt_tokens_total counter\n\
             prompt_tokens_total{model=\"alpha\"} 240\n\
             prompt_tokens_total{model=\"beta\"} 110\n\
             # TYPE latency_seconds histogram\n\
             latency_seconds_bucket{le=\"0.5\"} 12\n\
             latency_seconds_bucket{le=\"+Inf\"} 40\n\
             latency_seconds_sum 18\n\
             latency_seconds_count 40\n\
             # TYPE active gauge\n\
             active 3\n",
            CapacitySnapshot {
                enabled_lanes: 2,
                rpms: vec![40, 40],
                capacity_rpm: 80,
            },
        );
        history
    }

    fn value(values: &[MetricValue], name: &str) -> f64 {
        values
            .iter()
            .find_map(|metric| (metric.metric == name).then_some(metric.value))
            .unwrap()
    }

    fn labeled_value(values: &[MetricValue], name: &str, label: &str, label_value: &str) -> f64 {
        values
            .iter()
            .find_map(|metric| {
                (metric.metric == name
                    && metric.labels.get(label).map(String::as_str) == Some(label_value))
                .then_some(metric.value)
            })
            .unwrap()
    }

    #[test]
    fn rollup_totals_do_not_change_with_point_budget() {
        let h = history_with_points();
        let coarse = h.rollup(99, 300, 2);
        let fine = h.rollup(99, 300, 1000);
        assert_eq!(coarse.totals, fine.totals);
        assert_eq!(value(&coarse.totals, "requests_total"), 40.0);
        assert!(coarse.points.len() <= 2);
    }

    #[test]
    fn rollup_uses_open_closed_sample_boundaries() {
        let h = history_with_points();
        let rollup = h.rollup(100, 200, 20);
        assert_eq!(value(&rollup.totals, "requests_total"), 15.0);
    }

    #[test]
    fn rollup_preserves_grouped_labels() {
        let rollup = history_with_points().rollup(99, 300, 20);
        assert_eq!(
            labeled_value(&rollup.totals, "prompt_tokens_total", "model", "alpha"),
            240.0
        );
        assert_eq!(
            labeled_value(&rollup.totals, "prompt_tokens_total", "model", "beta"),
            110.0
        );
    }

    #[test]
    fn rollup_keeps_latest_gauges_and_histogram_buckets() {
        let rollup = history_with_points().rollup(99, 300, 20);
        assert_eq!(value(&rollup.latest, "active"), 3.0);
        assert_eq!(
            labeled_value(&rollup.totals, "latency_seconds_bucket", "le", "0.5"),
            12.0
        );
        assert_eq!(
            labeled_value(&rollup.totals, "latency_seconds_bucket", "le", "+Inf"),
            40.0
        );
    }

    #[test]
    fn rollup_empty_window_retains_only_available_bounds() {
        let rollup = history_with_points().rollup(300, 400, 20);
        assert_eq!(rollup.available_from, Some(100));
        assert_eq!(rollup.available_to, Some(300));
        assert_eq!(rollup.effective_from, None);
        assert_eq!(rollup.effective_to, None);
        assert!(rollup.totals.is_empty());
        assert!(rollup.latest.is_empty());
        assert!(rollup.points.is_empty());
    }

    #[test]
    fn rollup_reports_effective_and_available_bounds() {
        let rollup = history_with_points().rollup(0, 250, 20);
        assert_eq!(rollup.available_from, Some(100));
        assert_eq!(rollup.available_to, Some(300));
        assert_eq!(rollup.effective_from, Some(100));
        assert_eq!(rollup.effective_to, Some(200));
    }

    #[test]
    fn rollup_time_weights_capacity() {
        let rollup = history_with_points().rollup(99, 300, 2);
        let weighted_rpm = rollup
            .points
            .iter()
            .map(|point| {
                point.capacity.as_ref().unwrap().average_rpm * point.duration_seconds as f64
            })
            .sum::<f64>();
        let duration = rollup
            .points
            .iter()
            .map(|point| point.duration_seconds)
            .sum::<u64>();
        assert_eq!(duration, 201);
        assert!((weighted_rpm / duration as f64 - 16040.0 / 201.0).abs() < 1e-12);
        assert_eq!(
            rollup
                .points
                .last()
                .unwrap()
                .capacity
                .as_ref()
                .unwrap()
                .latest_rpms,
            [40, 40]
        );
    }

    #[test]
    fn tail_reports_only_metrics_since_the_latest_indexed_sample() {
        let history = history_with_points();
        let current = history.current(
            350,
            "# TYPE requests_total counter\n\
             requests_total 43\n\
             # TYPE active gauge\n\
             active 7\n",
        );
        assert_eq!(value(&current.tail.totals, "requests_total"), 3.0);
        assert_eq!(value(&current.metrics, "requests_total"), 43.0);
        assert_eq!(value(&current.metrics, "active"), 7.0);
        assert_eq!(current.tail.from, Some(300));
        assert_eq!(current.tail.to, 350);
        assert_eq!(current.tail.base_history_revision, history.revision());
    }

    #[test]
    fn tail_counts_a_new_process_value_from_zero() {
        let history = history_with_points();
        history.inner.lock().unwrap().last_sample_boot = Some("previous-process".to_owned());
        let current = history.current(350, "# TYPE requests_total counter\nrequests_total 2\n");
        assert_eq!(value(&current.tail.totals, "requests_total"), 2.0);
    }

    #[test]
    fn tail_polling_does_not_accumulate_previous_polls() {
        let history = history_with_points();
        let exposition = "# TYPE requests_total counter\nrequests_total 43\n";
        let first = history.current(350, exposition);
        let second = history.current(351, exposition);
        assert_eq!(value(&first.tail.totals, "requests_total"), 3.0);
        assert_eq!(value(&second.tail.totals, "requests_total"), 3.0);
        assert_eq!(history.revision(), 3);
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
        let h = History::load(Some(dir.0.clone()), 0, capacity(40));
        let all = h.range(0, u64::MAX, 100);
        assert_eq!(all.len(), 3, "3 valid lines parsed, junk skipped");
        assert_eq!(all[0].0, 5, "snapshots sorted by timestamp on load");
        // days > 0 exercises the "{days} days" retention log branch.
        let h2 = History::load(Some(dir.0.clone()), 7, capacity(40));
        assert_eq!(h2.range(0, u64::MAX, 100).len(), 3);
    }

    #[test]
    fn append_compacts_the_file_after_a_days_worth_of_expiry() {
        let dir = TestDir::new();
        let path = dir.0.join("history.jsonl");
        fs::write(&path, "{\"t\":1,\"m\":\"old\"}\n").unwrap();
        let h = history_with_file(1, path.clone());
        h.points.lock().unwrap().push(RawPoint {
            t: 1,
            m: "old".to_owned(),
            boot: None,
            capacity: None,
        });
        h.inner.lock().unwrap().points.push(IndexedPoint {
            t: 1,
            deltas: BTreeMap::new(),
            gauges: BTreeMap::new(),
            capacity: None,
        });
        // One more expiry crosses the >288 compaction threshold.
        *h.dropped_since_compact.lock().unwrap() = 288;
        // days = 1: cutoff = 200_000 - 86_400 = 113_600, so the t=1 snapshot
        // expires; that pushes the drop count to 289 (>288) and triggers a full
        // file rewrite rather than an append.
        h.append(200_000, "new", capacity(40));
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
        let h = History::load(Some(blocker.join("sub")), 1, capacity(40));
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
        let h = History::load(Some(dir.0.clone()), 1, capacity(40));
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
        let h = history_with_file(0, dir.0.clone());
        h.append(1, "snap", capacity(40));
        assert_eq!(
            h.range(0, u64::MAX, 10).len(),
            1,
            "in-memory append still works"
        );
    }
}
