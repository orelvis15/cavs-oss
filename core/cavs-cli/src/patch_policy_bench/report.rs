//! Report rendering for the patch policy benchmark (v1.1.0):
//! summary.md/json, patch_graph.json, CSV exports, storage/traffic/apply
//! chain reports and tool_versions.json.

use super::model::PatchGraph;
use super::simulate::{BudgetCandidate, ClientState, PolicySummary, QueryOutcome};
use super::traffic::{TrafficModel, WeightedQuery};
use crate::report::human_bytes;
use anyhow::Result;
use std::fmt::Write as _;
use std::path::Path;

pub struct ReportInputs<'a> {
    pub out: &'a Path,
    pub graph: &'a PatchGraph,
    pub summaries: &'a [PolicySummary],
    pub outcomes: &'a [QueryOutcome],
    pub model: &'a TrafficModel,
    pub queries: &'a [WeightedQuery],
    pub engine: &'a str,
    pub state: ClientState,
    pub budget: Option<&'a (u64, Vec<BudgetCandidate>)>,
    pub notes: &'a [String],
}

pub fn write_all(inputs: &ReportInputs) -> Result<()> {
    let out = inputs.out;
    std::fs::create_dir_all(out)?;

    std::fs::write(
        out.join("summary.md"),
        render_summary_md(
            inputs.graph,
            inputs.summaries,
            inputs.model,
            inputs.engine,
            inputs.state,
            inputs.notes,
        ),
    )?;
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "versions": inputs.graph.versions,
            "policies": inputs.summaries,
            "traffic_model": inputs.model,
            "engine": inputs.engine,
            "client_state": inputs.state.label(),
            "notes": inputs.notes,
        }))?,
    )?;
    std::fs::write(
        out.join("patch_graph.json"),
        serde_json::to_vec_pretty(inputs.graph)?,
    )?;
    std::fs::write(out.join("policy_edges.csv"), render_edges_csv(inputs.graph))?;
    std::fs::write(
        out.join("query_results.csv"),
        render_queries_csv(inputs.outcomes),
    )?;
    std::fs::write(
        out.join("storage_report.md"),
        render_storage_md(inputs.graph, inputs.summaries, inputs.budget),
    )?;
    std::fs::write(
        out.join("traffic_report.md"),
        render_traffic_md(inputs.graph, inputs.model, inputs.queries),
    )?;
    std::fs::write(
        out.join("apply_chain_report.md"),
        render_apply_md(inputs.summaries),
    )?;
    std::fs::write(
        out.join("tool_versions.json"),
        serde_json::to_vec_pretty(&inputs.graph.tool_versions)?,
    )?;
    Ok(())
}

pub fn render_summary_md(
    graph: &PatchGraph,
    summaries: &[PolicySummary],
    model: &TrafficModel,
    engine: &str,
    state: ClientState,
    notes: &[String],
) -> String {
    let mut md = String::new();
    let n = graph.versions.len();
    md.push_str("# Patch policy benchmark\n\n");
    let _ = writeln!(
        md,
        "{n} versions ({} … {}), pairwise engine **{engine}**, traffic model \
         **{}** ({} users), client state **{}**.\n",
        graph.versions[0].id,
        graph.versions[n - 1].id,
        model.name,
        model.users,
        state.label(),
    );
    md.push_str(
        "Pairwise diffs are not a single strategy. This benchmark compares several \
         practical patch graph policies: adjacent-only, sparse power-of-two ladder, \
         base-version, hot-pair, and all-pairs. The all-pairs graph is included only \
         as a theoretical one-hop baseline.\n\n",
    );
    md.push_str(
        "| Policy | Patch count | Storage | Avg update | P95 update | P99 update | Max steps | Build time | Coverage | Notes |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
    );
    for s in summaries {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {:.1}s | {:.1}% | {} |",
            s.label,
            if s.policy == "cavs" {
                "content store".to_string()
            } else {
                s.patch_count.to_string()
            },
            human_bytes(s.storage_bytes),
            human_bytes(s.avg_bytes),
            human_bytes(s.p95_bytes),
            human_bytes(s.p99_bytes),
            s.max_steps,
            s.build_ms as f64 / 1000.0,
            s.coverage * 100.0,
            s.notes,
        );
    }
    md.push_str(
        "\nStorage is the sum of stored patch bytes for the policy (deduplicated \
         chunk store for CAVS). Avg/P95/P99 update bytes are weighted by the \
         traffic model; uncovered queries fall back to a full compressed download \
         and count against coverage.\n",
    );
    if !notes.is_empty() {
        md.push_str("\n## Notes\n\n");
        for n in notes {
            let _ = writeln!(md, "- {n}");
        }
    }
    md
}

fn render_edges_csv(graph: &PatchGraph) -> String {
    let mut csv = String::from(
        "from,to,policies,engine,raw_patch_bytes,compressed_patch_bytes,diff_ms,apply_ms,verify_ms,peak_rss_mib,verified\n",
    );
    for edge in &graph.edges {
        for m in &edge.measures {
            let _ = writeln!(
                csv,
                "{},{},{},{},{},{},{},{},{},{},{}",
                graph.versions[edge.from].id,
                graph.versions[edge.to].id,
                edge.policies.join("+"),
                m.engine,
                m.raw_patch_bytes,
                m.compressed_patch_bytes,
                m.diff_ms,
                m.apply_ms,
                m.verify_ms,
                m.peak_rss_mib
                    .map(|r| format!("{r:.1}"))
                    .unwrap_or_default(),
                m.verified,
            );
        }
    }
    csv
}

fn render_queries_csv(outcomes: &[QueryOutcome]) -> String {
    let mut csv =
        String::from("policy,from,to,rule,probability,bytes,steps,apply_ms,verify_ms,covered\n");
    for o in outcomes {
        let _ = writeln!(
            csv,
            "{},{},{},{},{:.6},{},{},{},{},{}",
            o.policy,
            o.from,
            o.to,
            o.rule,
            o.probability,
            o.bytes,
            o.steps,
            o.apply_ms,
            o.verify_ms,
            o.covered,
        );
    }
    csv
}

fn render_storage_md(
    graph: &PatchGraph,
    summaries: &[PolicySummary],
    budget: Option<&(u64, Vec<BudgetCandidate>)>,
) -> String {
    let latest = graph.versions.last().unwrap();
    let mut md = String::from("# Storage report\n\n");
    let _ = writeln!(
        md,
        "Latest build: {} raw, {} compressed.\n",
        human_bytes(latest.size_bytes),
        human_bytes(latest.compressed_bytes),
    );
    md.push_str("| Policy | Patch count | Storage | Storage / latest build | Total served |\n");
    md.push_str("|---|---:|---:|---:|---:|\n");
    for s in summaries {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {:.2}× | {} |",
            s.label,
            if s.policy == "cavs" {
                "content store".into()
            } else {
                s.patch_count.to_string()
            },
            human_bytes(s.storage_bytes),
            s.storage_bytes as f64 / latest.compressed_bytes.max(1) as f64,
            human_bytes(s.total_served_bytes),
        );
    }
    if let Some((budget, candidates)) = budget {
        md.push_str("\n## Hot-pair storage budget\n\n");
        let _ = writeln!(
            md,
            "Budget: {}. Greedy selection by expected bytes saved per stored byte; \
             a patch is kept only when it beats its fallback route.\n",
            human_bytes(*budget)
        );
        md.push_str(
            "| Pair | Patch | Fallback route | Traffic share | Kept |\n|---|---:|---:|---:|---|\n",
        );
        for c in candidates {
            let _ = writeln!(
                md,
                "| {}→{} | {} | {} | {:.2}% | {} |",
                c.from,
                c.to,
                human_bytes(c.patch_bytes),
                human_bytes(c.fallback_bytes),
                c.expected_traffic * 100.0,
                if c.selected { "yes" } else { "no" },
            );
        }
    }
    md
}

fn render_traffic_md(
    graph: &PatchGraph,
    model: &TrafficModel,
    queries: &[WeightedQuery],
) -> String {
    let mut md = String::from("# Traffic report\n\n");
    let _ = writeln!(
        md,
        "Model **{}**, {} users, expanded to {} weighted (from,to) queries over {} versions.\n",
        model.name,
        model.users,
        queries.len(),
        graph.versions.len(),
    );
    md.push_str("| Rule | Probability |\n|---|---:|\n");
    for r in &model.rules {
        let detail = match r.kind.as_str() {
            "skip_range" => format!("skip_range {}–{}", r.min_skip, r.max_skip),
            "old_to_latest" => format!("old_to_latest (age ≥ {})", r.min_age),
            other => other.to_string(),
        };
        let _ = writeln!(md, "| {} | {:.0}% |", detail, r.probability * 100.0);
    }
    md.push_str("\n| From | To | Rule | Probability |\n|---|---|---|---:|\n");
    for q in queries {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {:.3}% |",
            graph.versions[q.from].id,
            graph.versions[q.to].id,
            q.rule,
            q.probability * 100.0,
        );
    }
    md
}

fn render_apply_md(summaries: &[PolicySummary]) -> String {
    let mut md = String::from("# Apply chain report\n\n");
    md.push_str(
        "Longer patch chains mean more sequential applies: more CPU, more \
         intermediate state, and a larger failure surface (every intermediate \
         patch must exist and apply cleanly).\n\n",
    );
    md.push_str(
        "| Policy | Avg steps | P95 steps | Max steps | Avg apply time |\n|---|---:|---:|---:|---:|\n",
    );
    for s in summaries {
        let _ = writeln!(
            md,
            "| {} | {:.2} | {} | {} | {} ms |",
            s.label, s.avg_steps, s.p95_steps, s.max_steps, s.avg_apply_ms,
        );
    }
    md.push_str(
        "\nCAVS routes and all-pairs patches are single-step by construction; \
         adjacent chains grow with the version distance; the ladder bounds chains \
         at O(log distance); base hubs need at most two steps but pay base drift.\n",
    );
    md
}

pub fn print_summary(
    summaries: &[PolicySummary],
    engine: &str,
    model: &TrafficModel,
    state: ClientState,
) {
    println!(
        "patch-policy: engine {engine} · traffic {} ({} users) · state {}",
        model.name,
        model.users,
        state.label()
    );
    println!(
        "  {:<44} {:>7} {:>12} {:>12} {:>12} {:>12} {:>6} {:>9} {:>9}",
        "policy",
        "patches",
        "storage",
        "avg update",
        "p95 update",
        "p99 update",
        "steps",
        "build",
        "coverage"
    );
    for s in summaries {
        println!(
            "  {:<44} {:>7} {:>12} {:>12} {:>12} {:>12} {:>6} {:>8.1}s {:>8.1}%",
            s.label,
            if s.policy == "cavs" {
                "store".into()
            } else {
                s.patch_count.to_string()
            },
            human_bytes(s.storage_bytes),
            human_bytes(s.avg_bytes),
            human_bytes(s.p95_bytes),
            human_bytes(s.p99_bytes),
            s.max_steps,
            s.build_ms as f64 / 1000.0,
            s.coverage * 100.0,
        );
    }
}
