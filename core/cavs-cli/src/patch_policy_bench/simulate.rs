//! Price every traffic query under every policy (v1.1.0).
//!
//! For pairwise policies a query costs the cheapest path through that
//! policy's edges (bytes, steps, apply time); uncovered queries fall
//! back to a full compressed download and are counted against coverage.
//! The CAVS route is priced from the chunk inventory: one step, bytes =
//! chunks the client doesn't already have.

use super::model::{cheapest_path, PatchGraph};
use super::traffic::WeightedQuery;
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// Cold cache, previous install present — the conservative default.
    ColdWithPreviousInstall,
    /// Persistent chunk cache warmed by earlier updates.
    WarmCache,
}

impl ClientState {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "cold-cache-with-previous-install" | "cold" => Ok(Self::ColdWithPreviousInstall),
            "warm-cache" | "warm" => Ok(Self::WarmCache),
            other => bail!(
                "unknown client state {other:?} \
                 (cold-cache-with-previous-install, warm-cache)"
            ),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ColdWithPreviousInstall => "cold-cache-with-previous-install",
            Self::WarmCache => "warm-cache",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryOutcome {
    pub from: String,
    pub to: String,
    pub rule: String,
    pub probability: f64,
    pub policy: String,
    pub bytes: u64,
    pub steps: usize,
    pub apply_ms: u64,
    pub verify_ms: u64,
    /// False when the policy had no path and full download was served.
    pub covered: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicySummary {
    pub policy: String,
    pub label: String,
    pub patch_count: usize,
    pub storage_bytes: u64,
    pub build_ms: u64,
    pub avg_bytes: u64,
    pub median_bytes: u64,
    pub p95_bytes: u64,
    pub p99_bytes: u64,
    pub worst_bytes: u64,
    pub avg_steps: f64,
    pub p95_steps: u64,
    pub max_steps: u64,
    pub avg_apply_ms: u64,
    pub total_served_bytes: u64,
    /// Probability mass served through the patch graph (vs full-download
    /// fallback). CAVS always covers 1.0 — no graph required.
    pub coverage: f64,
    pub notes: String,
}

/// ~500 MB/s reconstruct estimate, same as `plan-update` scoring.
fn est_apply_ms(bytes: u64) -> u64 {
    bytes / (500 * 1024 * 1024 / 1000)
}

pub fn simulate_policy(
    graph: &PatchGraph,
    policy_name: &str,
    engine: &str,
    queries: &[WeightedQuery],
    users: u64,
    state: ClientState,
) -> Result<(PolicySummary, Vec<QueryOutcome>)> {
    let n = graph.versions.len();
    let cavs = policy_name == "cavs";
    let spec = graph
        .policy(policy_name)
        .ok_or_else(|| anyhow::anyhow!("policy {policy_name:?} not in the graph"))?;

    let mut outcomes = Vec::with_capacity(queries.len());
    for q in queries {
        let id = |i: usize| graph.versions[i].id.clone();
        let outcome = if cavs {
            let routes = graph.cavs_routes.as_ref().ok_or_else(|| {
                anyhow::anyhow!("graph has no CAVS route data (regenerate with bench patch-policy)")
            })?;
            let (bytes, steps) = if q.from == q.to {
                (routes.install[q.to], 1)
            } else {
                let b = match state {
                    ClientState::ColdWithPreviousInstall => routes.cold[q.from][q.to],
                    ClientState::WarmCache => routes.warm[q.from][q.to],
                };
                (b, 1)
            };
            QueryOutcome {
                from: id(q.from),
                to: id(q.to),
                rule: q.rule.clone(),
                probability: q.probability,
                policy: policy_name.into(),
                bytes,
                steps,
                apply_ms: est_apply_ms(graph.versions[q.to].size_bytes),
                verify_ms: 0,
                covered: true,
            }
        } else if q.from == q.to {
            // reinstall: every pairwise policy serves the full build.
            QueryOutcome {
                from: id(q.from),
                to: id(q.to),
                rule: q.rule.clone(),
                probability: q.probability,
                policy: policy_name.into(),
                bytes: graph.versions[q.to].compressed_bytes,
                steps: 0,
                apply_ms: 0,
                verify_ms: 0,
                covered: true,
            }
        } else {
            let path = cheapest_path(&graph.edges, &spec.edge_idxs, n, q.from, q.to, &|e| {
                e.bytes(engine)
            });
            match path {
                Some(edges) if !edges.is_empty() => {
                    let mut bytes = 0u64;
                    let mut apply_ms = 0u64;
                    let mut verify_ms = 0u64;
                    for &e in &edges {
                        let m = graph.edges[e].measure(engine).ok_or_else(|| {
                            anyhow::anyhow!("edge without {engine} measurement in graph")
                        })?;
                        bytes += m.compressed_patch_bytes;
                        apply_ms += m.apply_ms;
                        verify_ms += m.verify_ms;
                    }
                    QueryOutcome {
                        from: id(q.from),
                        to: id(q.to),
                        rule: q.rule.clone(),
                        probability: q.probability,
                        policy: policy_name.into(),
                        bytes,
                        steps: edges.len(),
                        apply_ms,
                        verify_ms,
                        covered: true,
                    }
                }
                _ => QueryOutcome {
                    from: id(q.from),
                    to: id(q.to),
                    rule: q.rule.clone(),
                    probability: q.probability,
                    policy: policy_name.into(),
                    bytes: graph.versions[q.to].compressed_bytes,
                    steps: 0,
                    apply_ms: 0,
                    verify_ms: 0,
                    covered: false,
                },
            }
        };
        outcomes.push(outcome);
    }

    // ---- policy-level storage/build costs ---------------------------------
    let (patch_count, storage_bytes, build_ms) = if cavs {
        let routes = graph.cavs_routes.as_ref().unwrap();
        (0, routes.store_bytes, routes.build_ms)
    } else {
        let mut storage = 0u64;
        let mut build = 0u64;
        for &e in &spec.edge_idxs {
            if let Some(m) = graph.edges[e].measure(engine) {
                storage += m.compressed_patch_bytes;
                build += m.diff_ms;
            }
        }
        (spec.edge_idxs.len(), storage, build)
    };

    let summary = PolicySummary {
        policy: policy_name.into(),
        label: spec.label.clone(),
        patch_count,
        storage_bytes,
        build_ms,
        avg_bytes: weighted_avg(&outcomes, |o| o.bytes as f64) as u64,
        median_bytes: weighted_percentile(&outcomes, 0.50, |o| o.bytes),
        p95_bytes: weighted_percentile(&outcomes, 0.95, |o| o.bytes),
        p99_bytes: weighted_percentile(&outcomes, 0.99, |o| o.bytes),
        worst_bytes: outcomes.iter().map(|o| o.bytes).max().unwrap_or(0),
        avg_steps: weighted_avg(&outcomes, |o| o.steps as f64),
        p95_steps: weighted_percentile(&outcomes, 0.95, |o| o.steps as u64),
        max_steps: outcomes.iter().map(|o| o.steps as u64).max().unwrap_or(0),
        avg_apply_ms: weighted_avg(&outcomes, |o| o.apply_ms as f64) as u64,
        total_served_bytes: (users as f64 * weighted_avg(&outcomes, |o| o.bytes as f64)) as u64,
        coverage: outcomes
            .iter()
            .filter(|o| o.covered)
            .map(|o| o.probability)
            .sum(),
        notes: spec.notes.clone(),
    };
    Ok((summary, outcomes))
}

fn weighted_avg(outcomes: &[QueryOutcome], f: impl Fn(&QueryOutcome) -> f64) -> f64 {
    let total: f64 = outcomes.iter().map(|o| o.probability).sum();
    if total <= 0.0 {
        return 0.0;
    }
    outcomes.iter().map(|o| o.probability * f(o)).sum::<f64>() / total
}

fn weighted_percentile(outcomes: &[QueryOutcome], q: f64, f: impl Fn(&QueryOutcome) -> u64) -> u64 {
    let mut items: Vec<(u64, f64)> = outcomes.iter().map(|o| (f(o), o.probability)).collect();
    items.sort_by_key(|&(v, _)| v);
    let total: f64 = items.iter().map(|&(_, p)| p).sum();
    let mut acc = 0.0;
    for (v, p) in &items {
        acc += p;
        if acc >= q * total {
            return *v;
        }
    }
    items.last().map(|&(v, _)| v).unwrap_or(0)
}

// ---- storage budget optimizer ------------------------------------------------

/// Parse `1GiB` / `500MiB` / `2x-latest-build` / `0.5x-latest-build`.
pub fn parse_budget(spec: &str, latest_build_bytes: u64) -> Result<u64> {
    if let Some(mult) = spec.strip_suffix("x-latest-build") {
        let factor: f64 = mult
            .parse()
            .map_err(|_| anyhow::anyhow!("cannot parse budget multiplier in {spec:?}"))?;
        return Ok((factor * latest_build_bytes as f64) as u64);
    }
    crate::synth::parse_size_pub(spec)
}

/// A hot-pair candidate with measured cost and the bytes it saves over
/// the fallback route the client would otherwise take.
#[derive(Debug, Clone, Serialize)]
pub struct BudgetCandidate {
    pub edge_idx: usize,
    pub from: String,
    pub to: String,
    pub patch_bytes: u64,
    pub fallback_bytes: u64,
    pub expected_traffic: f64,
    pub selected: bool,
}

/// Greedy selection under a byte budget: order candidates by expected
/// bytes saved per stored byte, keep a patch only while it fits and
/// actually beats its fallback route.
pub fn select_under_budget(candidates: &mut [BudgetCandidate], budget: u64) {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    let score = |c: &BudgetCandidate| -> f64 {
        if c.patch_bytes == 0 {
            return 0.0;
        }
        let saved = c.fallback_bytes.saturating_sub(c.patch_bytes) as f64;
        c.expected_traffic * saved / c.patch_bytes as f64
    };
    order.sort_by(|&a, &b| score(&candidates[b]).total_cmp(&score(&candidates[a])));
    let mut spent = 0u64;
    for i in order {
        let c = &mut candidates[i];
        let saves = c.fallback_bytes > c.patch_bytes;
        if saves && spent + c.patch_bytes <= budget {
            c.selected = true;
            spent += c.patch_bytes;
        } else {
            c.selected = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(bytes: u64, steps: usize, p: f64) -> QueryOutcome {
        QueryOutcome {
            from: "a".into(),
            to: "b".into(),
            rule: "test".into(),
            probability: p,
            policy: "test".into(),
            bytes,
            steps,
            apply_ms: 0,
            verify_ms: 0,
            covered: true,
        }
    }

    #[test]
    fn weighted_percentiles_respect_probability_mass() {
        let outcomes = vec![
            outcome(100, 1, 0.90),
            outcome(1000, 5, 0.05),
            outcome(10_000, 9, 0.05),
        ];
        assert_eq!(weighted_percentile(&outcomes, 0.50, |o| o.bytes), 100);
        assert_eq!(weighted_percentile(&outcomes, 0.95, |o| o.bytes), 1000);
        assert_eq!(weighted_percentile(&outcomes, 0.99, |o| o.bytes), 10_000);
        let avg = weighted_avg(&outcomes, |o| o.bytes as f64);
        assert!((avg - (0.9 * 100.0 + 0.05 * 1000.0 + 0.05 * 10_000.0)).abs() < 1e-6);
    }

    #[test]
    fn budget_parse_handles_sizes_and_multiples() {
        assert_eq!(parse_budget("1GiB", 0).unwrap(), 1 << 30);
        assert_eq!(parse_budget("2x-latest-build", 100).unwrap(), 200);
        assert_eq!(parse_budget("0.5x-latest-build", 100).unwrap(), 50);
        assert!(parse_budget("nonsense-budget", 0).is_err());
    }

    #[test]
    fn greedy_selection_prefers_savings_per_byte_and_respects_budget() {
        let cand = |idx: usize, patch: u64, fallback: u64, traffic: f64| BudgetCandidate {
            edge_idx: idx,
            from: format!("v{idx}"),
            to: "vN".into(),
            patch_bytes: patch,
            fallback_bytes: fallback,
            expected_traffic: traffic,
            selected: false,
        };
        let mut candidates = vec![
            cand(0, 100, 1000, 0.5), // saves 900, score 4.5/byte-ish → best
            cand(1, 400, 500, 0.5),  // saves 100
            cand(2, 100, 50, 0.9),   // costs more than fallback → never
        ];
        select_under_budget(&mut candidates, 450);
        assert!(candidates[0].selected);
        assert!(!candidates[1].selected); // 100 + 400 exceeds the 450 budget
        assert!(!candidates[2].selected); // costs more than its fallback

        // with room for both, the useful ones are kept
        select_under_budget(&mut candidates, 500);
        assert!(candidates[0].selected && candidates[1].selected);
        assert!(!candidates[2].selected);
    }
}
