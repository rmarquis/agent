use crate::config;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsEntry {
    pub timestamp_ms: u64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub model: String,
}

fn metrics_path() -> PathBuf {
    config::state_dir().join("metrics.jsonl")
}

/// Append a single entry to the metrics JSONL file.
pub fn append(entry: &MetricsEntry) {
    let path = metrics_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if let Ok(line) = serde_json::to_string(entry) {
        let _ = writeln!(f, "{line}");
    }
}

/// Load all metrics entries from disk.
pub fn load() -> Vec<MetricsEntry> {
    let path = metrics_path();
    let Ok(f) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    std::io::BufReader::new(f)
        .lines()
        .filter_map(|line| {
            let line = line.ok()?;
            serde_json::from_str(&line).ok()
        })
        .collect()
}

// ── Aggregation ─────────────────────────────────────────────────────────────

fn day_key(ms: u64) -> u64 {
    ms / (24 * 3600 * 1000)
}

fn hour_key(ms: u64) -> u64 {
    ms / (3600 * 1000)
}

struct Stats {
    total_calls: usize,
    total_prompt: u64,
    total_completion: u64,
    by_model: BTreeMap<String, (u64, u64, usize)>,
    by_day: BTreeMap<u64, u64>,
    by_hour: BTreeMap<u64, u64>,
}

fn aggregate(entries: &[MetricsEntry]) -> Stats {
    let mut stats = Stats {
        total_calls: entries.len(),
        total_prompt: 0,
        total_completion: 0,
        by_model: BTreeMap::new(),
        by_day: BTreeMap::new(),
        by_hour: BTreeMap::new(),
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let h24_ago = now_ms.saturating_sub(24 * 3600 * 1000);

    for e in entries {
        let prompt = e.prompt_tokens as u64;
        let completion = e.completion_tokens as u64;
        let total = prompt + completion;

        stats.total_prompt += prompt;
        stats.total_completion += completion;

        let m = stats.by_model.entry(e.model.clone()).or_insert((0, 0, 0));
        m.0 += prompt;
        m.1 += completion;
        m.2 += 1;

        *stats.by_day.entry(day_key(e.timestamp_ms)).or_insert(0) += total;

        if e.timestamp_ms >= h24_ago {
            *stats.by_hour.entry(hour_key(e.timestamp_ms)).or_insert(0) += total;
        }
    }

    stats
}

// ── Structured output for the renderer ──────────────────────────────────────

pub enum StatsLine {
    /// Dim label + accent value.
    Kv { label: String, value: String },
    /// Sub-item (indented, all dim).
    Sub(String),
    /// Section heading (accent).
    Heading(String),
    /// Sparkline bar characters (rendered in accent).
    Sparkline { bars: String, legend: String },
    /// One row of the daily heatmap.
    HeatRow { label: String, cells: Vec<HeatCell> },
    /// Empty separator line.
    Blank,
}

#[derive(Clone, Copy)]
pub enum HeatCell {
    Empty,
    /// Intensity 0..=3 (maps to increasing brightness).
    Level(u8),
}

const SPARKLINE: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn sparkline(values: &[u64]) -> String {
    let max = values.iter().copied().max().unwrap_or(1).max(1);
    values
        .iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * (SPARKLINE.len() - 1) as f64).round() as usize;
            SPARKLINE[idx.min(SPARKLINE.len() - 1)]
        })
        .collect()
}

pub fn render_stats(entries: &[MetricsEntry]) -> Vec<StatsLine> {
    if entries.is_empty() {
        return vec![StatsLine::Sub("No metrics recorded yet.".into())];
    }

    let stats = aggregate(entries);
    let mut lines = Vec::new();
    let total = stats.total_prompt + stats.total_completion;

    lines.push(StatsLine::Kv {
        label: "calls".into(),
        value: stats.total_calls.to_string(),
    });
    lines.push(StatsLine::Kv {
        label: "tokens".into(),
        value: format!(
            "{} ({} prompt + {} completion)",
            fmt(total),
            fmt(stats.total_prompt),
            fmt(stats.total_completion),
        ),
    });
    if stats.total_calls > 0 {
        lines.push(StatsLine::Kv {
            label: "avg/call".into(),
            value: format!("{} tokens", fmt(total / stats.total_calls as u64)),
        });
    }

    // Per-model breakdown
    if stats.by_model.len() > 1 {
        lines.push(StatsLine::Blank);
        lines.push(StatsLine::Heading("per model".into()));
        for (model, (prompt, completion, calls)) in &stats.by_model {
            lines.push(StatsLine::Sub(format!(
                "{model}: {calls} calls, {} tokens",
                fmt(prompt + completion),
            )));
        }
    }

    // Last 24h hourly sparkline
    if !stats.by_hour.is_empty() {
        lines.push(StatsLine::Blank);
        lines.push(StatsLine::Heading("last 24 hours".into()));

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now_hour = hour_key(now_ms);
        let values: Vec<u64> = (0..24)
            .map(|i| {
                let h = now_hour - 23 + i;
                stats.by_hour.get(&h).copied().unwrap_or(0)
            })
            .collect();
        let bars = sparkline(&values);
        lines.push(StatsLine::Sparkline {
            bars,
            legend: "24h ago ─────────────── now".into(),
        });
    }

    // Daily heatmap (last 12 weeks)
    if !stats.by_day.is_empty() {
        lines.push(StatsLine::Blank);
        lines.push(StatsLine::Heading("daily activity (12 weeks)".into()));

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let today = day_key(now_ms);
        let days: Vec<u64> = (0..84).map(|i| today - 83 + i).collect();
        let values: Vec<u64> = days
            .iter()
            .map(|d| stats.by_day.get(d).copied().unwrap_or(0))
            .collect();
        let max = values.iter().copied().max().unwrap_or(1).max(1);

        let day_labels = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
        for (row, label) in day_labels.iter().enumerate() {
            let mut cells = Vec::new();
            for week in 0..12 {
                let idx = week * 7 + row;
                if idx < values.len() {
                    let v = values[idx];
                    if v == 0 {
                        cells.push(HeatCell::Empty);
                    } else {
                        let level = ((v as f64 / max as f64) * 3.0).round() as u8;
                        cells.push(HeatCell::Level(level.min(3)));
                    }
                }
            }
            lines.push(StatsLine::HeatRow {
                label: label.to_string(),
                cells,
            });
        }
    }

    lines
}

fn fmt(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
