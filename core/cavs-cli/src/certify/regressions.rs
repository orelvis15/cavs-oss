//! `cavs certify regressions` — compare this run's metrics against a
//! recorded baseline and fail when thresholds are exceeded.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

use super::{worst, CheckResult, CheckRow, BASELINE_SCHEMA};
use crate::report::human_bytes;

pub struct Thresholds {
    pub network: f64,
    pub apply: f64,
    pub ram: f64,
}

impl Thresholds {
    pub fn parse(network: &str, apply: &str, ram: &str) -> Result<Thresholds> {
        Ok(Thresholds {
            network: parse_pct(network)?,
            apply: parse_pct(apply)?,
            ram: parse_pct(ram)?,
        })
    }

    /// Threshold applied to a metric, by name: times use the apply
    /// threshold, RAM uses the RAM threshold, everything byte-sized uses
    /// the network threshold.
    fn for_metric(&self, name: &str) -> f64 {
        if name.ends_with("_ms") {
            self.apply
        } else if name.contains("ram") {
            self.ram
        } else {
            self.network
        }
    }
}

/// Absolute delta a metric must also exceed before it can fail: wall-clock
/// and RSS measurements jitter run-to-run, so a relative threshold alone
/// would flag noise on small workloads. Byte counts are exact — no floor.
fn noise_floor(name: &str) -> f64 {
    if name.ends_with("_ms") {
        250.0
    } else if name.contains("ram") {
        32.0 * 1024.0 * 1024.0
    } else {
        0.0
    }
}

fn parse_pct(s: &str) -> Result<f64> {
    let t = s.trim().trim_end_matches('%');
    let v: f64 = t
        .parse()
        .with_context(|| format!("CAVS-E-CERTIFY-INPUT: bad threshold '{s}' (expected e.g. 5%)"))?;
    if !(0.0..=1000.0).contains(&v) {
        bail!("CAVS-E-CERTIFY-INPUT: threshold '{s}' out of range");
    }
    Ok(v / 100.0)
}

#[derive(serde::Serialize)]
pub struct MetricDelta {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
    pub change_pct: f64,
    pub threshold_pct: f64,
    pub status: CheckResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
}

pub struct Outcome {
    pub rows: Vec<CheckRow>,
    pub result: CheckResult,
    pub deltas: Vec<MetricDelta>,
}

/// Read a metrics map (+ byte_identical flag) from a baseline file, a
/// certify `routes.json`/`summary.json`, or any JSON with a top-level
/// `metrics` object.
pub fn load_metrics(path: &Path) -> Result<(BTreeMap<String, f64>, bool)> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("CAVS-E-CERTIFY-INPUT: {} is not JSON", path.display()))?;
    let metrics_val = value
        .get("metrics")
        .cloned()
        .unwrap_or_else(|| value.clone());
    let obj = metrics_val.as_object().with_context(|| {
        format!(
            "CAVS-E-CERTIFY-INPUT: {} has no metrics object",
            path.display()
        )
    })?;
    let mut metrics = BTreeMap::new();
    for (k, v) in obj {
        if let Some(n) = v.as_f64() {
            metrics.insert(k.clone(), n);
        }
    }
    if metrics.is_empty() {
        bail!(
            "CAVS-E-CERTIFY-INPUT: {} contains no numeric metrics",
            path.display()
        );
    }
    let byte_identical = value
        .get("byte_identical")
        .and_then(|v| v.as_bool())
        .or_else(|| metrics.get("byte_identical").map(|v| *v != 0.0))
        .unwrap_or(true);
    Ok((metrics, byte_identical))
}

fn parse_exceptions(allow: &[String]) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for spec in allow {
        let (metric, reason) = spec.split_once('=').with_context(|| {
            format!("CAVS-E-CERTIFY-INPUT: --allow-regression '{spec}' needs metric=reason")
        })?;
        if reason.trim().is_empty() {
            bail!("CAVS-E-CERTIFY-INPUT: --allow-regression '{spec}' needs an explicit reason");
        }
        map.insert(metric.trim().to_string(), reason.trim().to_string());
    }
    Ok(map)
}

pub fn compare(
    current: &BTreeMap<String, f64>,
    current_byte_identical: bool,
    baseline: &BTreeMap<String, f64>,
    baseline_byte_identical: bool,
    thresholds: &Thresholds,
    allow: &[String],
) -> Result<Outcome> {
    let exceptions = parse_exceptions(allow)?;
    let mut rows: Vec<CheckRow> = Vec::new();
    let mut deltas: Vec<MetricDelta> = Vec::new();

    // Byte-identical status may never regress, and has no exceptions.
    rows.push(if baseline_byte_identical && !current_byte_identical {
        CheckRow::new(
            "byte-identical status",
            CheckResult::Fail,
            "baseline was byte-identical; this run is not",
        )
    } else {
        CheckRow::new(
            "byte-identical status",
            CheckResult::Pass,
            format!("current: {current_byte_identical}"),
        )
    });

    for (name, cur) in current {
        if name == "byte_identical" {
            continue;
        }
        let Some(base) = baseline.get(name) else {
            continue; // new metric: informational, not comparable
        };
        if *base <= 0.0 {
            continue;
        }
        let change = (cur - base) / base;
        let threshold = thresholds.for_metric(name);
        let exception = exceptions.get(name).cloned();
        let status = if change <= threshold || (cur - base) <= noise_floor(name) {
            CheckResult::Pass
        } else if let Some(reason) = &exception {
            rows.push(CheckRow::new(
                &format!("exception: {name}"),
                CheckResult::Warn,
                format!("+{:.1}% accepted — {}", change * 100.0, reason),
            ));
            CheckResult::Warn
        } else {
            CheckResult::Fail
        };
        if status == CheckResult::Fail {
            rows.push(CheckRow::new(
                &format!("metric: {name}"),
                CheckResult::Fail,
                format!(
                    "+{:.1}% exceeds the {:.0}% threshold ({} → {})",
                    change * 100.0,
                    threshold * 100.0,
                    show(name, *base),
                    show(name, *cur)
                ),
            ));
        }
        deltas.push(MetricDelta {
            metric: name.clone(),
            baseline: *base,
            current: *cur,
            change_pct: change * 100.0,
            threshold_pct: threshold * 100.0,
            status,
            exception,
        });
    }
    if deltas.is_empty() {
        rows.push(CheckRow::new(
            "comparable metrics",
            CheckResult::Warn,
            "no metric present in both current and baseline",
        ));
    } else {
        rows.push(CheckRow::new(
            "comparable metrics",
            CheckResult::Pass,
            format!("{} metrics compared", deltas.len()),
        ));
    }

    let result = worst(&rows);
    Ok(Outcome {
        rows,
        result,
        deltas,
    })
}

fn show(name: &str, v: f64) -> String {
    if name.ends_with("_bytes") {
        human_bytes(v as u64)
    } else if name.ends_with("_ms") {
        format!("{v:.0} ms")
    } else {
        format!("{v}")
    }
}

#[derive(serde::Serialize)]
struct BaselineFile<'a> {
    schema: &'static str,
    cavs_version: &'static str,
    byte_identical: bool,
    metrics: &'a BTreeMap<String, f64>,
}

pub fn write_baseline(
    metrics: &BTreeMap<String, f64>,
    byte_identical: bool,
    path: &Path,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&BaselineFile {
            schema: BASELINE_SCHEMA,
            cavs_version: env!("CARGO_PKG_VERSION"),
            byte_identical,
            metrics,
        })?,
    )?;
    Ok(())
}

#[derive(serde::Serialize)]
struct Report<'a> {
    schema: &'static str,
    result: CheckResult,
    deltas: &'a [MetricDelta],
    checks: &'a [CheckRow],
}

pub fn write_reports(outcome: &Outcome, out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(
        out_dir.join("regressions.json"),
        serde_json::to_vec_pretty(&Report {
            schema: "cavs-certify-regressions/1",
            result: outcome.result,
            deltas: &outcome.deltas,
            checks: &outcome.rows,
        })?,
    )?;
    let mut md = String::from("# Regression Report\n\n");
    md.push_str(&format!("Result: **{}**\n\n", outcome.result.label()));
    md.push_str("| Metric | Baseline | Current | Change | Threshold | Status |\n");
    md.push_str("|---|---:|---:|---:|---:|---|\n");
    for d in &outcome.deltas {
        md.push_str(&format!(
            "| {} | {} | {} | {:+.1}% | {:.0}% | {}{} |\n",
            d.metric,
            show(&d.metric, d.baseline),
            show(&d.metric, d.current),
            d.change_pct,
            d.threshold_pct,
            d.status.label(),
            d.exception
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ));
    }
    md.push('\n');
    md.push_str(&super::rows_markdown(&outcome.rows));
    md.push_str(
        "\nByte counts are exact and compared strictly against their threshold. \
         Timing (`*_ms`) and RAM metrics additionally need an absolute delta \
         (>250 ms / >32 MiB) before failing: single-run wall-clock jitter is \
         not a regression.\n",
    );
    std::fs::write(out_dir.join("regressions.md"), md)?;
    Ok(())
}
