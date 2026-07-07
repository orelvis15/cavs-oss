//! `cavs certify routes` — the route planner must recommend correct,
//! available and verified routes for every documented client state, and
//! (when tools allow) every measured route must reconstruct byte-identical
//! output.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{worst, CheckResult, CheckRow};
use crate::bench_routes::{self, RoutesReport};
use crate::plan_update::{self, ClientState, PlanUpdateArgs, ScoredRoute};
use crate::report::human_bytes;

/// The documented client-state matrix: certification name → planner tokens.
pub const DEFAULT_STATES: &[(&str, &str)] = &[
    ("cold-install", "cold-install"),
    ("cold-cache-previous", "cold-cache,has-previous-install"),
    ("warm-cache", "warm-cache,has-previous-install"),
    ("exact-previous-version", "has-previous-install"),
    ("low-ram", "has-previous-install,low-ram"),
    ("slow-hdd", "has-previous-install,slow-hdd"),
    ("limited-disk", "has-previous-install,low-disk"),
];

pub struct Args<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    /// Pre-generated `.cavsplan` for this pair (built here when absent).
    pub plan: Option<&'a Path>,
    pub client_states: Option<&'a str>,
    pub policy: &'a str,
    /// Run the measured route matrix (real applies, external tools).
    pub measured: bool,
    pub butler_bin: Option<&'a str>,
    /// Byte-identical verdict from the integrity phase, when already known.
    pub byte_identical: Option<bool>,
}

#[derive(serde::Serialize)]
pub struct StateDecision {
    pub state: String,
    pub planner_tokens: String,
    pub chosen: String,
    pub reason: String,
    pub network_bytes: u64,
    pub apply_ms: u64,
    pub peak_ram_bytes: u64,
    pub routes: Vec<ScoredRoute>,
}

pub struct Outcome {
    pub rows: Vec<CheckRow>,
    pub result: CheckResult,
    pub recommended: String,
    pub reason: String,
    pub reasons: Vec<String>,
    pub states: Vec<StateDecision>,
    pub measured: Option<RoutesReport>,
    pub metrics: BTreeMap<String, f64>,
    pub policy: String,
    pub weights: BTreeMap<String, f64>,
    pub byte_identical: Option<bool>,
}

fn resolve_states(spec: Option<&str>) -> Vec<(String, String)> {
    match spec {
        None => DEFAULT_STATES
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect(),
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|name| {
                let tokens = DEFAULT_STATES
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, t)| t.to_string())
                    .unwrap_or_else(|| name.to_string());
                (name.to_string(), tokens)
            })
            .collect(),
    }
}

/// Ensure a `.cavsplan` exists for the pair; build one when absent.
fn ensure_plan(args: &Args, out_dir: &Path, commands: &mut Vec<String>) -> Result<PathBuf> {
    if let Some(p) = args.plan {
        return Ok(p.to_path_buf());
    }
    let artifacts = out_dir.join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    let plan_path = artifacts.join("update.cavsplan");
    if plan_path.exists() {
        return Ok(plan_path);
    }
    crate::diff_plan::diff_plan(&crate::diff_plan::DiffPlanArgs {
        old: Some(args.old),
        old_signature: None,
        new: args.new,
        out: &plan_path,
        analysis: false,
        block_kib: 64,
        zstd_level: 19,
        report: None,
    })?;
    commands.push(format!(
        "cavs diff-plan {} {} --out artifacts/update.cavsplan",
        args.old.display(),
        args.new.display()
    ));
    Ok(plan_path)
}

pub fn run(args: &Args, out_dir: &Path, commands: &mut Vec<String>) -> Result<Outcome> {
    std::fs::create_dir_all(out_dir)?;
    let pol = plan_update::policy(args.policy)?;
    let plan_path = ensure_plan(args, out_dir, commands)?;

    let mut rows: Vec<CheckRow> = Vec::new();
    let mut states: Vec<StateDecision> = Vec::new();
    let mut metrics: BTreeMap<String, f64> = BTreeMap::new();

    // -- Planner decisions per client state ---------------------------------
    for (name, tokens) in resolve_states(args.client_states) {
        let state = ClientState::parse(&tokens)
            .with_context(|| format!("client state '{name}' ({tokens})"))?;
        let has_previous = state.has_previous_install;
        let plan_args = PlanUpdateArgs {
            from: if has_previous { Some(args.old) } else { None },
            to: args.new,
            plan_file: if has_previous { Some(&plan_path) } else { None },
            patch_file: None,
            bootstrap_file: None,
            client_state: &tokens,
            policy: args.policy,
            json: false,
        };
        commands.push(format!(
            "cavs plan-update{} --to {} --client-state '{}' --policy {} --json",
            if has_previous {
                format!(" --from {}", args.old.display())
            } else {
                String::new()
            },
            args.new.display(),
            tokens,
            args.policy
        ));
        let mut routes = plan_update::collect_routes(&plan_args, &state)?;
        let (chosen, reason) = plan_update::score_and_choose(&mut routes, &pol, &state)?;

        let chosen_route = routes.iter().find(|r| r.route == chosen);
        let row = match chosen_route {
            None => CheckRow::new(
                &format!("state: {name}"),
                CheckResult::Fail,
                format!("chosen route '{chosen}' is not among the candidates"),
            ),
            Some(r) if !r.available => CheckRow::new(
                &format!("state: {name}"),
                CheckResult::Fail,
                format!("chosen route '{chosen}' is unavailable — {}", r.notes),
            ),
            Some(r) => {
                // A cold install must never depend on a previous install.
                let previous_dependent = chosen.contains("plan")
                    || chosen.contains("hybrid")
                    || chosen.contains("no-op")
                    || chosen.contains("sidecar");
                if !has_previous && previous_dependent {
                    CheckRow::new(
                        &format!("state: {name}"),
                        CheckResult::Fail,
                        format!("'{chosen}' needs a previous install this state lacks"),
                    )
                } else {
                    CheckRow::new(
                        &format!("state: {name}"),
                        CheckResult::Pass,
                        format!(
                            "{chosen} — {} network, {} ms apply ({reason})",
                            human_bytes(r.network_bytes),
                            r.apply_ms
                        ),
                    )
                }
            }
        };
        rows.push(row);
        let (net, ms, ram) = chosen_route
            .map(|r| (r.network_bytes, r.apply_ms, r.peak_ram_bytes))
            .unwrap_or_default();
        states.push(StateDecision {
            state: name,
            planner_tokens: tokens,
            chosen,
            reason,
            network_bytes: net,
            apply_ms: ms,
            peak_ram_bytes: ram,
            routes,
        });
    }
    if states.is_empty() {
        bail!("CAVS-E-CERTIFY-INPUT: no client states to certify");
    }

    // -- Measured route matrix -----------------------------------------------
    let mut measured: Option<RoutesReport> = None;
    if args.measured {
        let bench_dir = out_dir.join("artifacts").join("route-bench");
        std::fs::create_dir_all(&bench_dir)?;
        commands.push(format!(
            "cavs bench routes --old {} --new {}{} --include-pairwise-proxy --out artifacts/route-bench",
            args.old.display(),
            args.new.display(),
            args.butler_bin
                .map(|b| format!(" --butler-bin {b}"))
                .unwrap_or_default()
        ));
        let report = bench_routes::collect(&bench_routes::RoutesArgs {
            old: args.old,
            new: args.new,
            butler_bin: args.butler_bin,
            include_pairwise_proxy: true,
            out: &bench_dir,
        })?;
        std::fs::write(
            out_dir.join("artifacts").join("route-results.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        let broken: Vec<&str> = report
            .routes
            .iter()
            .filter(|r| r.output_ok == Some(false))
            .map(|r| r.route.as_str())
            .collect();
        rows.push(if broken.is_empty() {
            CheckRow::new(
                "measured routes verified",
                CheckResult::Pass,
                format!(
                    "{} routes measured, {} skipped (missing tools)",
                    report.routes.len(),
                    report.skipped.len()
                ),
            )
        } else {
            CheckRow::new(
                "measured routes verified",
                CheckResult::Fail,
                format!("routes with non-identical output: {}", broken.join(", ")),
            )
        });
        for skipped in &report.skipped {
            rows.push(CheckRow::new(
                &format!("skipped: {skipped}"),
                CheckResult::Skipped,
                "optional dependency not installed — skipped, never selected",
            ));
        }
        measured = Some(report);
    }

    // -- Recommendation ---------------------------------------------------------
    let (recommended, reason_line, reasons) = recommend(&states, measured.as_ref(), args);

    // -- Metrics -------------------------------------------------------------------
    if let Some(m) = &measured {
        if let Some(plan_row) = m.routes.iter().find(|r| r.route.contains("cavsplan")) {
            metrics.insert("network_bytes".into(), plan_row.network_bytes as f64);
            if let Some(ms) = plan_row.apply_ms {
                metrics.insert("apply_ms".into(), ms as f64);
            }
            if let Some(ms) = plan_row.diff_ms {
                metrics.insert("diff_ms".into(), ms as f64);
            }
            if let Some(rss) = plan_row.peak_rss_mib {
                metrics.insert("peak_ram_bytes".into(), rss * 1024.0 * 1024.0);
            }
        }
        metrics.insert("full_download_bytes".into(), m.new_size_bytes as f64);
    } else if let Some(s) = states.iter().find(|s| s.state == "exact-previous-version") {
        metrics.insert("network_bytes".into(), s.network_bytes as f64);
        metrics.insert("apply_ms".into(), s.apply_ms as f64);
        metrics.insert("peak_ram_bytes".into(), s.peak_ram_bytes as f64);
    }

    let weights: BTreeMap<String, f64> = BTreeMap::from([
        ("network".into(), pol.network),
        ("apply_ms".into(), pol.apply_ms),
        ("ram_mb".into(), pol.ram_mb),
        ("temp_disk".into(), pol.temp_disk),
        ("disk_read".into(), pol.disk_read),
        ("build_ms".into(), pol.build_ms),
    ]);

    let result = worst(&rows);
    Ok(Outcome {
        rows,
        result,
        recommended,
        reason: reason_line,
        reasons,
        states,
        measured,
        metrics,
        policy: args.policy.to_string(),
        weights,
        byte_identical: args.byte_identical,
    })
}

/// Smallest verified network payload wins; `.cavsplan` breaks near-ties
/// (same rule as `cavs publish-preview`).
fn recommend(
    states: &[StateDecision],
    measured: Option<&RoutesReport>,
    args: &Args,
) -> (String, String, Vec<String>) {
    if let Some(m) = measured {
        let candidates: Vec<&bench_routes::RouteRow> = m
            .routes
            .iter()
            .filter(|r| r.output_ok != Some(false))
            .collect();
        if let Some(min) = candidates.iter().map(|r| r.network_bytes).min() {
            let near: Vec<&&bench_routes::RouteRow> = candidates
                .iter()
                .filter(|r| r.network_bytes <= min + min / 50)
                .collect();
            if let Some(best) = near
                .iter()
                .find(|r| r.route.contains("cavsplan"))
                .or_else(|| near.first())
            {
                let mut reasons = vec![format!(
                    "{} network — the smallest verified payload",
                    human_bytes(best.network_bytes)
                )];
                if let Some(ms) = best.apply_ms {
                    reasons.push(format!("{ms} ms apply"));
                }
                if best.route.contains("cavsplan") {
                    reasons.push("streaming memory (no full old copy in RAM)".into());
                }
                if args.byte_identical == Some(true) {
                    reasons.push("byte-identical output".into());
                }
                return (best.route.clone(), reasons.join("; "), reasons);
            }
        }
    }
    // Estimate-only fallback: the exact-previous-version planner decision.
    if let Some(s) = states
        .iter()
        .find(|s| s.state == "exact-previous-version")
        .or_else(|| states.first())
    {
        return (s.chosen.clone(), s.reason.clone(), vec![s.reason.clone()]);
    }
    ("full download".into(), "no candidates".into(), Vec::new())
}

#[derive(serde::Serialize)]
struct Report<'a> {
    schema: &'static str,
    result: CheckResult,
    policy: &'a str,
    weights: &'a BTreeMap<String, f64>,
    recommended: &'a str,
    reasons: &'a [String],
    states: &'a [StateDecision],
    #[serde(skip_serializing_if = "Option::is_none")]
    measured: &'a Option<RoutesReport>,
    metrics: &'a BTreeMap<String, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    byte_identical: Option<bool>,
    checks: &'a [CheckRow],
}

pub fn write_reports(outcome: &Outcome, out_dir: &Path) -> Result<()> {
    let report = Report {
        schema: "cavs-certify-routes/1",
        result: outcome.result,
        policy: &outcome.policy,
        weights: &outcome.weights,
        recommended: &outcome.recommended,
        reasons: &outcome.reasons,
        states: &outcome.states,
        measured: &outcome.measured,
        metrics: &outcome.metrics,
        byte_identical: outcome.byte_identical,
        checks: &outcome.rows,
    };
    std::fs::write(
        out_dir.join("routes.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;

    let mut md = String::from("# Route Certification\n\n");
    md.push_str(&format!("Result: **{}**\n\n", outcome.result.label()));
    md.push_str(&format!(
        "Policy: `{}` — weights: {}\n\n",
        outcome.policy,
        outcome
            .weights
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    md.push_str("## Decisions per client state\n\n");
    md.push_str("| Client state | Recommended route | Network | Apply | RAM | Reason |\n");
    md.push_str("|---|---|---:|---:|---:|---|\n");
    for s in &outcome.states {
        md.push_str(&format!(
            "| {} | {} | {} | {} ms | {} | {} |\n",
            s.state,
            s.chosen,
            human_bytes(s.network_bytes),
            s.apply_ms,
            human_bytes(s.peak_ram_bytes),
            s.reason.replace('|', "\\|")
        ));
    }
    md.push('\n');
    if let Some(m) = &outcome.measured {
        md.push_str("## Measured route matrix\n\n");
        md.push_str(&bench_routes::markdown(m));
        md.push('\n');
    }
    md.push_str("## Checks\n\n");
    md.push_str(&super::rows_markdown(&outcome.rows));
    md.push_str(&format!(
        "\nRecommended route: **{}**\n\nWhy: {}\n",
        outcome.recommended, outcome.reason
    ));
    md.push_str(
        "\nRules: a route is never chosen when its dependency is unavailable, \
         when it fails verification, or when it exceeds the policy limits; \
         near-ties prefer the simpler, lower-risk route.\n",
    );
    std::fs::write(out_dir.join("routes.md"), md)?;
    Ok(())
}
