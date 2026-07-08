//! `cavs bench patch-policy` and the `cavs patch-policy graph|simulate|
//! explain` family (v1.1.0).
//!
//! Pairwise diffs are not a single strategy. This benchmark measures the
//! practical patch graph policies real systems deploy — adjacent-only,
//! sparse power-of-two ladders, base-version hubs, hot pairs under a
//! storage budget — against the all-pairs one-hop baseline (kept only as
//! the theoretical bound) and against CAVS content-addressed routes,
//! under an explicit user traffic model. CAVS is not trying to win by
//! dismissing pairwise patching; it prices the same queries under every
//! policy and reports where each one is the better tradeoff.

pub mod engines;
pub mod model;
pub mod report;
pub mod simulate;
pub mod traffic;

use anyhow::{bail, Context, Result};
use model::{
    adjacent_pairs, all_pairs, base_pairs, hot_pairs_latest_k, ladder_pairs, CavsRoutes,
    LadderMode, PatchEdge, PatchGraph, PolicySpec, VersionNode,
};
use simulate::{BudgetCandidate, ClientState, PolicySummary, QueryOutcome};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const DEFAULT_POLICIES: &str = "adjacent,ladder,base,hot-pairs,all-pairs,cavs";
pub const DEFAULT_ENGINES: &str = "cavsplan,bsdiff,xdelta3";

pub struct BenchArgs {
    pub versions: Vec<PathBuf>,
    pub versions_dir: Option<PathBuf>,
    pub version_glob: String,
    pub sort: String,
    pub policies: String,
    pub patch_engines: String,
    pub traffic_model: String,
    pub users: Option<u64>,
    pub client_state: String,
    pub ladder_mode: String,
    pub base_policy: String,
    pub hot_pairs: String,
    pub patch_storage_budget: Option<String>,
    pub compression: String,
    pub keep_patches: bool,
    pub out: PathBuf,
}

pub struct GraphArgs {
    pub versions: Vec<PathBuf>,
    pub versions_dir: Option<PathBuf>,
    pub version_glob: String,
    pub sort: String,
    pub policies: String,
    pub ladder_mode: String,
    pub base_policy: String,
    pub hot_pairs: String,
    pub out: PathBuf,
}

// ---- version discovery -------------------------------------------------------

fn collect_versions(
    versions: &[PathBuf],
    versions_dir: Option<&Path>,
    glob: &str,
    sort: &str,
) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = if !versions.is_empty() {
        versions.to_vec()
    } else if let Some(dir) = versions_dir {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("cannot read --versions-dir {}", dir.display()))?
        {
            let path = entry?.path();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') || !glob_match(glob, &name) {
                continue;
            }
            found.push(path);
        }
        found
    } else {
        bail!("pass either --versions <paths…> or --versions-dir <dir>");
    };
    for p in &paths {
        if !p.exists() {
            bail!("version path {} does not exist", p.display());
        }
    }
    match sort {
        "name" => paths.sort_by_key(|p| version_id(p)),
        "semver" => paths.sort_by_key(|p| (numeric_key(&version_id(p)), version_id(p))),
        "mtime" => paths.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        }),
        other => bail!("unknown --sort {other:?} (name, semver, mtime)"),
    }
    if paths.len() < 2 {
        bail!(
            "need at least two versions, found {} (dir/glob too narrow?)",
            paths.len()
        );
    }
    Ok(paths)
}

/// `*` and `?` only — enough for `v*` / `build-?` patterns.
fn glob_match(pattern: &str, name: &str) -> bool {
    fn inner(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], n) || (!n.is_empty() && inner(p, &n[1..])),
            (Some(b'?'), Some(_)) => inner(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => inner(&p[1..], &n[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}

fn version_id(path: &Path) -> String {
    if path.is_dir() {
        path.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// Numeric components of an id, for semver-ish ordering (v1.2.10 > v1.2.9).
fn numeric_key(id: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut current = String::new();
    for c in id.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            key.push(current.parse().unwrap_or(0));
            current.clear();
        }
    }
    if !current.is_empty() {
        key.push(current.parse().unwrap_or(0));
    }
    key
}

fn tree_size(path: &Path) -> Result<u64> {
    if path.is_dir() {
        let mut total = 0;
        for (_, abs) in cavs_analyzer::walk::walk(path)? {
            total += std::fs::metadata(&abs)?.len();
        }
        Ok(total)
    } else {
        Ok(std::fs::metadata(path)?.len())
    }
}

/// zstd-3 of every file — the "full download" price of a build.
fn compressed_size(path: &Path) -> Result<u64> {
    let files: Vec<PathBuf> = if path.is_dir() {
        cavs_analyzer::walk::walk(path)?
            .into_iter()
            .map(|(_, abs)| abs)
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    let mut total = 0u64;
    for f in files {
        total += zstd::bulk::compress(&std::fs::read(&f)?, 3)?.len() as u64;
    }
    Ok(total)
}

fn build_nodes(paths: &[PathBuf], measure: bool) -> Result<Vec<VersionNode>> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            Ok(VersionNode {
                id: version_id(path),
                index,
                path: path.display().to_string(),
                size_bytes: tree_size(path)?,
                compressed_bytes: if measure { compressed_size(path)? } else { 0 },
                signature_hash: hex(&engines::tree_hash(path)?),
            })
        })
        .collect()
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- policy construction -------------------------------------------------------

struct PolicyOptions {
    ladder_mode: LadderMode,
    base_policy: String,
    hot_pairs: String,
}

/// Hot-pair spec: `latest:K` or `traffic-top:K`.
enum HotSpec {
    LatestK(usize),
    TrafficTopK(usize),
}

fn parse_hot_spec(s: &str) -> Result<HotSpec> {
    if let Some(k) = s.strip_prefix("latest:") {
        return Ok(HotSpec::LatestK(k.parse().context("bad latest:K")?));
    }
    if let Some(k) = s.strip_prefix("traffic-top:") {
        return Ok(HotSpec::TrafficTopK(
            k.parse().context("bad traffic-top:K")?,
        ));
    }
    bail!("unknown --hot-pairs {s:?} (latest:K, traffic-top:K)")
}

/// Base candidates: explicit index, or several to test for `auto`.
fn base_candidates(nodes: &[VersionNode], policy: &str) -> Result<Vec<usize>> {
    let n = nodes.len();
    let find = |id: &str| {
        nodes
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.index)
            .ok_or_else(|| anyhow::anyhow!("--base-policy fixed:{id}: no such version"))
    };
    Ok(match policy {
        "first" => vec![0],
        "middle" => vec![n / 2],
        "latest-major" => vec![latest_major(nodes)],
        "auto" => {
            let mut c = vec![0, n / 2, latest_major(nodes)];
            c.sort();
            c.dedup();
            c
        }
        fixed if fixed.starts_with("fixed:") => vec![find(&fixed[6..])?],
        other => {
            bail!("unknown --base-policy {other:?} (first, middle, latest-major, fixed:<id>, auto)")
        }
    })
}

/// Most recent version whose leading numeric component increased —
/// a major-release checkpoint heuristic; falls back to the first version.
fn latest_major(nodes: &[VersionNode]) -> usize {
    let major = |v: &VersionNode| numeric_key(&v.id).first().copied().unwrap_or(0);
    (1..nodes.len())
        .rev()
        .find(|&i| major(&nodes[i]) > major(&nodes[i - 1]))
        .unwrap_or(0)
}

/// Build the edge pool and the policy specs. Edges are deduplicated by
/// (from,to); a shared edge is measured once and tagged with every
/// policy that uses it.
fn build_graph_structure(
    nodes: &[VersionNode],
    requested: &[String],
    opts: &PolicyOptions,
    traffic_queries: Option<&[traffic::WeightedQuery]>,
) -> Result<(Vec<PatchEdge>, Vec<PolicySpec>)> {
    let n = nodes.len();
    let mut pool: HashMap<(usize, usize), usize> = HashMap::new();
    let mut edges: Vec<PatchEdge> = Vec::new();
    let mut specs: Vec<PolicySpec> = Vec::new();

    let add = |edges: &mut Vec<PatchEdge>,
               pool: &mut HashMap<(usize, usize), usize>,
               policy: &str,
               pairs: &[(usize, usize)]|
     -> Vec<usize> {
        let mut idxs = Vec::with_capacity(pairs.len());
        for &(from, to) in pairs {
            let idx = *pool.entry((from, to)).or_insert_with(|| {
                edges.push(PatchEdge {
                    from,
                    to,
                    policies: Vec::new(),
                    measures: Vec::new(),
                });
                edges.len() - 1
            });
            if !edges[idx].policies.iter().any(|p| p == policy) {
                edges[idx].policies.push(policy.to_string());
            }
            idxs.push(idx);
        }
        idxs.sort();
        idxs.dedup();
        idxs
    };

    for name in requested {
        match name.as_str() {
            "adjacent" => {
                let idxs = add(&mut edges, &mut pool, "adjacent", &adjacent_pairs(n));
                specs.push(PolicySpec {
                    name: "adjacent".into(),
                    label: "adjacent pairwise diffs".into(),
                    edge_idxs: idxs,
                    notes: "O(N) storage; skips chain patches".into(),
                });
            }
            "ladder" => {
                let pairs = ladder_pairs(n, opts.ladder_mode);
                let idxs = add(&mut edges, &mut pool, "ladder", &pairs);
                specs.push(PolicySpec {
                    name: "ladder".into(),
                    label: format!("sparse dyadic ladder ({})", opts.ladder_mode.label()),
                    edge_idxs: idxs,
                    notes: "<2N storage (aligned); O(log distance) chains".into(),
                });
            }
            "base" => {
                for &b in &base_candidates(nodes, &opts.base_policy)? {
                    let pairs = base_pairs(n, b, true);
                    let cname = format!("base-candidate:{}", nodes[b].id);
                    let idxs = add(&mut edges, &mut pool, &cname, &pairs);
                    specs.push(PolicySpec {
                        name: cname,
                        label: format!("base hub ({}, bidirectional)", nodes[b].id),
                        edge_idxs: idxs,
                        notes: "old→base→new, 2 steps max; base drift matters".into(),
                    });
                }
            }
            "hot-pairs" => {
                let latest = n - 1;
                let pairs = match parse_hot_spec(&opts.hot_pairs)? {
                    HotSpec::LatestK(k) => hot_pairs_latest_k(n, k),
                    HotSpec::TrafficTopK(k) => {
                        let queries = traffic_queries.ok_or_else(|| {
                            anyhow::anyhow!("--hot-pairs traffic-top:K needs a traffic model")
                        })?;
                        let mut ranked: Vec<&traffic::WeightedQuery> = queries
                            .iter()
                            .filter(|q| q.to != q.from && q.to - q.from > 1)
                            .collect();
                        ranked.sort_by(|a, b| b.probability.total_cmp(&a.probability));
                        let mut pairs = adjacent_pairs(n);
                        pairs.extend(ranked.iter().take(k).map(|q| (q.from, q.to)));
                        pairs.sort();
                        pairs.dedup();
                        pairs
                    }
                };
                let _ = latest;
                let idxs = add(&mut edges, &mut pool, "hot-pairs", &pairs);
                specs.push(PolicySpec {
                    name: "hot-pairs".into(),
                    label: format!("hot pairs ({}) + adjacent baseline", opts.hot_pairs),
                    edge_idxs: idxs,
                    notes: "budgeted direct patches for expected-hot jumps".into(),
                });
            }
            "all-pairs" => {
                let idxs = add(&mut edges, &mut pool, "all-pairs", &all_pairs(n));
                specs.push(PolicySpec {
                    name: "all-pairs".into(),
                    label: "all-pairs theoretical one-hop baseline".into(),
                    edge_idxs: idxs,
                    notes: "O(N²) storage; not a normal production policy".into(),
                });
            }
            "cavs" => {
                specs.push(PolicySpec {
                    name: "cavs".into(),
                    label: "CAVS content-addressed route".into(),
                    edge_idxs: Vec::new(),
                    notes: "no patch graph; chunk store serves any jump".into(),
                });
            }
            other => bail!("unknown policy {other:?} (available: {DEFAULT_POLICIES})"),
        }
    }
    Ok((edges, specs))
}

fn parse_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn parse_zstd_level(s: &str) -> Result<i32> {
    match s.strip_prefix("zstd-") {
        Some(level) => level.parse().context("bad zstd level"),
        None => bail!("unknown --compression {s:?} (zstd-N)"),
    }
}

// ---- bench: measure + simulate + report ----------------------------------------

pub fn bench(args: &BenchArgs) -> Result<()> {
    let paths = collect_versions(
        &args.versions,
        args.versions_dir.as_deref(),
        &args.version_glob,
        &args.sort,
    )?;
    let nodes = build_nodes(&paths, true)?;
    let n = nodes.len();
    eprintln!(
        "[patch-policy] {} versions ({} … {})",
        n,
        nodes[0].id,
        nodes[n - 1].id
    );

    let requested = parse_list(&args.policies);
    if requested.is_empty() {
        bail!("--policies is empty");
    }
    let zstd_level = parse_zstd_level(&args.compression)?;
    let state = ClientState::parse(&args.client_state)?;
    let mut model = traffic::load(&args.traffic_model)?;
    if let Some(users) = args.users {
        model.users = users;
    }
    let queries = traffic::expand(&model, n)?;

    let opts = PolicyOptions {
        ladder_mode: LadderMode::parse(&args.ladder_mode)?,
        base_policy: args.base_policy.clone(),
        hot_pairs: args.hot_pairs.clone(),
    };
    let (mut edges, mut specs) = build_graph_structure(&nodes, &requested, &opts, Some(&queries))?;

    // ---- engines: availability, then measure the pool ----------------------
    let engine_list = parse_list(&args.patch_engines);
    let mut notes: Vec<String> = Vec::new();
    let available: Vec<String> = engines::availability(&engine_list)
        .into_iter()
        .filter_map(|(engine, status)| match status {
            Ok(()) => Some(engine),
            Err(why) => {
                notes.push(format!("engine {engine} skipped: {why}"));
                eprintln!("[patch-policy] engine {engine} skipped: {why}");
                None
            }
        })
        .collect();
    if available.is_empty() {
        bail!("no patch engine available (cavsplan is built in — include it in --patch-engines)");
    }

    std::fs::create_dir_all(&args.out)?;
    let total_edges = edges.len();
    for (i, edge) in edges.iter_mut().enumerate() {
        let (from, to) = (edge.from, edge.to);
        for engine in &available {
            let keep_dir = if args.keep_patches {
                Some(
                    args.out
                        .join("raw")
                        .join(engine)
                        .join(format!("{}_to_{}", nodes[from].id, nodes[to].id)),
                )
            } else {
                None
            };
            match engines::measure_edge(
                engine,
                &paths[from],
                &paths[to],
                zstd_level,
                keep_dir.as_deref(),
            ) {
                Ok(m) => edge.measures.push(m),
                Err(err) => notes.push(format!(
                    "{engine} {}→{}: {err:#}",
                    nodes[from].id, nodes[to].id
                )),
            }
        }
        eprintln!(
            "[patch-policy] edge {}/{} {}→{} measured",
            i + 1,
            total_edges,
            nodes[from].id,
            nodes[to].id
        );
    }

    // Report engine: first requested engine with a verified measure on
    // every pairwise edge; cavsplan is the always-available fallback.
    let report_engine = available
        .iter()
        .find(|e| {
            edges
                .iter()
                .all(|edge| edge.measures.iter().any(|m| m.engine == **e && m.verified))
        })
        .cloned()
        .unwrap_or_else(|| "cavsplan".to_string());

    // ---- CAVS routes ---------------------------------------------------------
    let cavs_routes = if requested.iter().any(|p| p == "cavs") {
        eprintln!("[patch-policy] building CAVS chunk inventory…");
        let inv = engines::cavs_inventory(&paths)?;
        let mut cold = vec![vec![0u64; n]; n];
        let mut warm = vec![vec![0u64; n]; n];
        for from in 0..n {
            for to in from + 1..n {
                cold[from][to] = inv.update_bytes(from, to);
                warm[from][to] = inv.warm_update_bytes(from, to);
            }
        }
        Some(CavsRoutes {
            store_bytes: inv.store_bytes(),
            build_ms: inv.build_ms,
            cold,
            warm,
            install: (0..n).map(|v| inv.install_bytes(v)).collect(),
        })
    } else {
        None
    };

    let mut graph = PatchGraph {
        versions: nodes.clone(),
        edges,
        policies: specs.clone(),
        cavs_routes,
        structure_only: false,
        tool_versions: engines::tool_versions(&available),
    };

    // ---- hot-pairs storage budget --------------------------------------------
    let mut budget_report: Option<(u64, Vec<BudgetCandidate>)> = None;
    if let (Some(budget_spec), Some(pos)) = (
        args.patch_storage_budget.as_deref(),
        specs.iter().position(|s| s.name == "hot-pairs"),
    ) {
        let latest_build = nodes[n - 1].compressed_bytes;
        let budget = simulate::parse_budget(budget_spec, latest_build)?;
        let (kept, candidates) =
            apply_hot_pair_budget(&graph, pos, &report_engine, &queries, budget)?;
        graph.policies[pos].edge_idxs = kept.clone();
        graph.policies[pos].notes = format!(
            "budget {budget_spec} ({}); {} of {} hot edges kept",
            crate::report::human_bytes(budget),
            candidates.iter().filter(|c| c.selected).count(),
            candidates.len(),
        );
        specs[pos] = graph.policies[pos].clone();
        budget_report = Some((budget, candidates));
    }

    // ---- base auto selection ---------------------------------------------------
    resolve_base_policy(
        &mut graph,
        &requested,
        &report_engine,
        &queries,
        &model,
        state,
    )?;

    // ---- simulate every requested policy ---------------------------------------
    let mut summaries: Vec<PolicySummary> = Vec::new();
    let mut all_outcomes: Vec<QueryOutcome> = Vec::new();
    for name in &requested {
        let (summary, outcomes) =
            simulate::simulate_policy(&graph, name, &report_engine, &queries, model.users, state)?;
        summaries.push(summary);
        all_outcomes.extend(outcomes);
    }

    // ---- reports ------------------------------------------------------------------
    report::write_all(&report::ReportInputs {
        out: &args.out,
        graph: &graph,
        summaries: &summaries,
        outcomes: &all_outcomes,
        model: &model,
        queries: &queries,
        engine: &report_engine,
        state,
        budget: budget_report.as_ref(),
        notes: &notes,
    })?;
    report::print_summary(&summaries, &report_engine, &model, state);
    println!("results : {}/summary.md + summary.json", args.out.display());
    Ok(())
}

/// Measure hot-pair candidates against their fallback chain and keep the
/// winners under the budget. Returns (kept edge idxs, candidate report).
fn apply_hot_pair_budget(
    graph: &PatchGraph,
    spec_pos: usize,
    engine: &str,
    queries: &[traffic::WeightedQuery],
    budget: u64,
) -> Result<(Vec<usize>, Vec<BudgetCandidate>)> {
    let spec = &graph.policies[spec_pos];
    let n = graph.versions.len();
    let adjacent: Vec<usize> = spec
        .edge_idxs
        .iter()
        .copied()
        .filter(|&e| graph.edges[e].to - graph.edges[e].from == 1)
        .collect();
    let hot: Vec<usize> = spec
        .edge_idxs
        .iter()
        .copied()
        .filter(|&e| graph.edges[e].to - graph.edges[e].from > 1)
        .collect();

    let mut candidates: Vec<BudgetCandidate> = Vec::new();
    for &e in &hot {
        let edge = &graph.edges[e];
        let patch_bytes = edge.bytes(engine).unwrap_or(u64::MAX);
        let fallback =
            model::cheapest_path(&graph.edges, &adjacent, n, edge.from, edge.to, &|ed| {
                ed.bytes(engine)
            })
            .map(|path| {
                path.iter()
                    .filter_map(|&i| graph.edges[i].bytes(engine))
                    .sum::<u64>()
            })
            .unwrap_or_else(|| graph.versions[edge.to].compressed_bytes);
        let traffic_share: f64 = queries
            .iter()
            .filter(|q| q.from == edge.from && q.to == edge.to)
            .map(|q| q.probability)
            .sum();
        candidates.push(BudgetCandidate {
            edge_idx: e,
            from: graph.versions[edge.from].id.clone(),
            to: graph.versions[edge.to].id.clone(),
            patch_bytes,
            fallback_bytes: fallback,
            expected_traffic: traffic_share,
            selected: false,
        });
    }
    simulate::select_under_budget(&mut candidates, budget);
    let mut kept = adjacent;
    kept.extend(candidates.iter().filter(|c| c.selected).map(|c| c.edge_idx));
    kept.sort();
    kept.dedup();
    Ok((kept, candidates))
}

/// For `--base-policy auto` the graph carries one candidate spec per
/// base; simulate each and promote the cheapest (expected bytes under
/// the traffic model) to the canonical "base" policy name.
fn resolve_base_policy(
    graph: &mut PatchGraph,
    requested: &[String],
    engine: &str,
    queries: &[traffic::WeightedQuery],
    model: &traffic::TrafficModel,
    state: ClientState,
) -> Result<()> {
    if !requested.iter().any(|p| p == "base") {
        return Ok(());
    }
    let candidates: Vec<String> = graph
        .policies
        .iter()
        .filter(|s| s.name.starts_with("base-candidate:"))
        .map(|s| s.name.clone())
        .collect();
    if candidates.is_empty() {
        bail!("--policies includes base but no base candidate was built");
    }
    let mut best: Option<(String, u64)> = None;
    for name in &candidates {
        let (summary, _) =
            simulate::simulate_policy(graph, name, engine, queries, model.users, state)?;
        let cost = summary.avg_bytes;
        if best.as_ref().is_none_or(|(_, b)| cost < *b) {
            best = Some((name.clone(), cost));
        }
    }
    let (winner, _) = best.unwrap();
    let losers: Vec<String> = candidates
        .iter()
        .filter(|c| **c != winner)
        .cloned()
        .collect();
    for spec in &mut graph.policies {
        if spec.name == winner {
            spec.name = "base".into();
            if candidates.len() > 1 {
                spec.notes = format!(
                    "auto-selected over {} under the {} traffic model",
                    losers
                        .iter()
                        .map(|l| l.trim_start_matches("base-candidate:"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    model.name
                );
            }
        }
    }
    Ok(())
}

// ---- graph / simulate / explain -------------------------------------------------

pub fn graph_cmd(args: &GraphArgs) -> Result<()> {
    let paths = collect_versions(
        &args.versions,
        args.versions_dir.as_deref(),
        &args.version_glob,
        &args.sort,
    )?;
    let nodes = build_nodes(&paths, false)?;
    let requested = parse_list(&args.policies);
    let opts = PolicyOptions {
        ladder_mode: LadderMode::parse(&args.ladder_mode)?,
        base_policy: args.base_policy.clone(),
        hot_pairs: args.hot_pairs.clone(),
    };
    let (edges, specs) = build_graph_structure(&nodes, &requested, &opts, None)?;
    let graph = PatchGraph {
        versions: nodes,
        edges,
        policies: specs,
        cavs_routes: None,
        structure_only: true,
        tool_versions: Default::default(),
    };
    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&args.out, serde_json::to_vec_pretty(&graph)?)?;
    println!(
        "patch graph: {} versions, {} edges across {} policies → {}",
        graph.versions.len(),
        graph.edges.len(),
        graph.policies.len(),
        args.out.display()
    );
    for spec in &graph.policies {
        println!(
            "  {:<28} {:>5} edges  {}",
            spec.name,
            spec.edge_idxs.len(),
            spec.notes
        );
    }
    println!("(structure only — run `cavs bench patch-policy` to measure sizes)");
    Ok(())
}

fn load_graph(path: &Path) -> Result<PatchGraph> {
    let bytes =
        std::fs::read(path).with_context(|| format!("cannot read graph {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("bad graph JSON {}", path.display()))
}

pub fn simulate_cmd(
    graph_path: &Path,
    traffic_model: &str,
    users: Option<u64>,
    client_state: &str,
    out: Option<&Path>,
) -> Result<()> {
    let graph = load_graph(graph_path)?;
    if graph.structure_only {
        bail!(
            "graph {} is structure-only (from `patch-policy graph`); \
             run `cavs bench patch-policy` to measure patch sizes first",
            graph_path.display()
        );
    }
    let mut model = traffic::load(traffic_model)?;
    if let Some(users) = users {
        model.users = users;
    }
    let state = ClientState::parse(client_state)?;
    let queries = traffic::expand(&model, graph.versions.len())?;
    let engine = graph
        .tool_versions
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "cavsplan".into());
    let engine = if graph
        .edges
        .iter()
        .all(|e| e.measures.iter().any(|m| m.engine == engine && m.verified))
    {
        engine
    } else {
        "cavsplan".to_string()
    };

    let mut summaries = Vec::new();
    for spec in &graph.policies {
        if spec.name.starts_with("base-candidate:") {
            continue;
        }
        let (summary, _) =
            simulate::simulate_policy(&graph, &spec.name, &engine, &queries, model.users, state)?;
        summaries.push(summary);
    }
    report::print_summary(&summaries, &engine, &model, state);
    if let Some(out) = out {
        let md = report::render_summary_md(&graph, &summaries, &model, &engine, state, &[]);
        std::fs::write(out, md)?;
        println!("written : {}", out.display());
    }
    Ok(())
}

pub fn explain_cmd(graph_path: &Path, from: &str, to: &str, policy: &str) -> Result<()> {
    let graph = load_graph(graph_path)?;
    let spec = graph.policy(policy).ok_or_else(|| {
        anyhow::anyhow!(
            "policy {policy:?} not in the graph (available: {})",
            graph
                .policies
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let find = |id: &str| {
        graph
            .versions
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.index)
            .ok_or_else(|| anyhow::anyhow!("version {id:?} not in the graph"))
    };
    let (from_idx, to_idx) = (find(from)?, find(to)?);

    if policy == "cavs" {
        let routes = graph
            .cavs_routes
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("graph has no CAVS route data"))?;
        println!("CAVS route {from}→{to}:");
        println!(
            "  cold cache + previous install : {}",
            crate::report::human_bytes(routes.cold[from_idx][to_idx])
        );
        println!(
            "  warm cache                    : {}",
            crate::report::human_bytes(routes.warm[from_idx][to_idx])
        );
        println!("  1 step, content-addressed — no per-pair patch required");
        return Ok(());
    }

    let engine = graph
        .edges
        .iter()
        .flat_map(|e| e.measures.iter())
        .map(|m| m.engine.clone())
        .next();
    let bytes_of =
        |e: &PatchEdge| -> Option<u64> { engine.as_deref().and_then(|eng| e.bytes(eng)) };
    let path = model::cheapest_path(
        &graph.edges,
        &spec.edge_idxs,
        graph.versions.len(),
        from_idx,
        to_idx,
        &bytes_of,
    )
    .ok_or_else(|| {
        anyhow::anyhow!("{policy} has no path {from}→{to} (falls back to full download)")
    })?;

    println!("{} path {from}→{to}:", spec.label);
    let mut total = 0u64;
    for &e in &path {
        let edge = &graph.edges[e];
        let bytes = bytes_of(edge);
        total += bytes.unwrap_or(0);
        println!(
            "  {}→{}  {}",
            graph.versions[edge.from].id,
            graph.versions[edge.to].id,
            bytes
                .map(crate::report::human_bytes)
                .unwrap_or_else(|| "unmeasured".into())
        );
    }
    println!(
        "\nTotal:\n  {}\n  {} steps",
        crate::report::human_bytes(total),
        path.len()
    );
    Ok(())
}

// ---- gen-stream: synthetic many-version dataset -----------------------------------

/// Write a deterministic v01…vNN release stream: `drift_pct`% of 64 KiB
/// blocks change per release (cumulative); `major_at` rewrites 60% of
/// blocks at that release to model a major content/layout change.
pub fn gen_stream(
    out: &Path,
    size: &str,
    versions: usize,
    seed: u64,
    drift_pct: u32,
    major_at: Option<usize>,
) -> Result<()> {
    const BLOCK: usize = 64 * 1024;
    let total = crate::synth::parse_size_pub(size)?;
    let blocks = total.div_ceil(BLOCK as u64).max(4);
    let versions = versions.clamp(2, 200);
    std::fs::create_dir_all(out)?;

    let changed_at: Vec<std::collections::HashSet<u64>> = (1..versions)
        .map(|k| {
            let mut rng = crate::synth::Rng::new(seed.wrapping_mul(131).wrapping_add(k as u64));
            let pct = if major_at == Some(k) {
                60
            } else {
                drift_pct.clamp(1, 100)
            };
            let target = (blocks * pct as u64 / 100).max(1);
            let mut set = std::collections::HashSet::new();
            while (set.len() as u64) < target {
                set.insert(rng.next() % blocks);
            }
            set
        })
        .collect();
    let salt_of = |v: usize, i: u64| -> u64 {
        (1..=v)
            .rev()
            .find(|&k| changed_at[k - 1].contains(&i))
            .unwrap_or(0) as u64
    };

    let width = if versions >= 100 { 3 } else { 2 };
    for v in 0..versions {
        let path = out.join(format!("v{:0width$}.bin", v + 1, width = width));
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for i in 0..blocks {
            file.write_all(&crate::synth::block_bytes(seed, salt_of(v, i), i))?;
        }
        file.flush()?;
        eprintln!("[gen-stream] {} written", path.display());
    }
    println!(
        "gen-stream: {} versions × {} in {} ({}% drift per release{})",
        versions,
        crate::report::human_bytes(blocks * BLOCK as u64),
        out.display(),
        drift_pct,
        major_at
            .map(|k| format!(", major change at release {}", k + 1))
            .unwrap_or_default()
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = crate::synth::Rng::new(seed);
    let mut out = vec![0u8; len];
    for chunk in out.chunks_mut(8) {
        let v = rng.next().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(dir: &Path, n: usize) -> Vec<PathBuf> {
        gen_stream(dir, "1MiB", n, 5, 5, None).unwrap();
        (1..=n).map(|v| dir.join(format!("v{v:02}.bin"))).collect()
    }

    #[test]
    fn glob_and_sort_discover_versions() {
        let dir = tempfile::tempdir().unwrap();
        let paths = stream(dir.path(), 3);
        let found = collect_versions(&[], Some(dir.path()), "v*", "semver").unwrap();
        assert_eq!(found, paths);
        assert!(collect_versions(&[], Some(dir.path()), "zzz*", "name").is_err());
        assert!(glob_match("v*", "v01.bin"));
        assert!(!glob_match("v*", "build.bin"));
        assert!(glob_match("v??.bin", "v01.bin"));
    }

    #[test]
    fn semver_sort_orders_numerically() {
        assert!(numeric_key("v10") > numeric_key("v9"));
        assert_eq!(numeric_key("v1.2.10"), vec![1, 2, 10]);
    }

    #[test]
    fn end_to_end_bench_with_cavsplan_only() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("builds");
        stream(&data, 4);
        let out = dir.path().join("results");
        bench(&BenchArgs {
            versions: vec![],
            versions_dir: Some(data),
            version_glob: "v*".into(),
            sort: "semver".into(),
            policies: DEFAULT_POLICIES.into(),
            patch_engines: "cavsplan".into(),
            traffic_model: "adjacent-heavy".into(),
            users: Some(1000),
            client_state: "cold-cache-with-previous-install".into(),
            ladder_mode: "aligned".into(),
            base_policy: "auto".into(),
            hot_pairs: "latest:2".into(),
            patch_storage_budget: Some("2x-latest-build".into()),
            compression: "zstd-19".into(),
            keep_patches: false,
            out: out.clone(),
        })
        .unwrap();

        for file in [
            "summary.md",
            "summary.json",
            "patch_graph.json",
            "policy_edges.csv",
            "query_results.csv",
            "storage_report.md",
            "traffic_report.md",
            "apply_chain_report.md",
            "tool_versions.json",
        ] {
            assert!(out.join(file).exists(), "missing {file}");
        }

        // the graph round-trips and simulate/explain work on it
        let graph = load_graph(&out.join("patch_graph.json")).unwrap();
        assert!(graph.cavs_routes.is_some());
        assert!(graph.policy("base").is_some(), "auto base was promoted");
        simulate_cmd(
            &out.join("patch_graph.json"),
            "skip-heavy",
            None,
            "warm-cache",
            None,
        )
        .unwrap();
        explain_cmd(&out.join("patch_graph.json"), "v01", "v04", "ladder").unwrap();
        explain_cmd(&out.join("patch_graph.json"), "v01", "v04", "cavs").unwrap();

        // sanity: summary.json has every requested policy and sane shapes
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        let policies = summary["policies"].as_array().unwrap();
        assert_eq!(policies.len(), 6);
        let all_pairs = policies
            .iter()
            .find(|p| p["policy"] == "all-pairs")
            .unwrap();
        assert_eq!(all_pairs["patch_count"].as_u64().unwrap(), 6); // 4 versions → 6 pairs
        assert!(all_pairs["label"]
            .as_str()
            .unwrap()
            .contains("theoretical one-hop baseline"));
        let adjacent = policies.iter().find(|p| p["policy"] == "adjacent").unwrap();
        assert_eq!(adjacent["max_steps"].as_u64().unwrap(), 3);
        assert_eq!(all_pairs["max_steps"].as_u64().unwrap(), 1);
    }

    #[test]
    fn structure_only_graph_declines_simulation() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("builds");
        stream(&data, 3);
        let graph_path = dir.path().join("graph.json");
        graph_cmd(&GraphArgs {
            versions: vec![],
            versions_dir: Some(data),
            version_glob: "v*".into(),
            sort: "semver".into(),
            policies: "adjacent,ladder,all-pairs".into(),
            ladder_mode: "aligned".into(),
            base_policy: "auto".into(),
            hot_pairs: "latest:3".into(),
            out: graph_path.clone(),
        })
        .unwrap();
        let graph = load_graph(&graph_path).unwrap();
        assert!(graph.structure_only);
        assert!(graph.policy("all-pairs").unwrap().edge_idxs.len() == 3);
        let err = simulate_cmd(&graph_path, "adjacent-heavy", None, "cold", None).unwrap_err();
        assert!(err.to_string().contains("structure-only"));
    }

    #[test]
    fn gen_stream_is_deterministic_and_drifts() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        gen_stream(&a, "512KiB", 3, 9, 5, None).unwrap();
        gen_stream(&b, "512KiB", 3, 9, 5, None).unwrap();
        for v in 1..=3 {
            let name = format!("v0{v}.bin");
            assert_eq!(
                std::fs::read(a.join(&name)).unwrap(),
                std::fs::read(b.join(&name)).unwrap(),
                "{name} differs between identical seeds"
            );
        }
        assert_ne!(
            std::fs::read(a.join("v01.bin")).unwrap(),
            std::fs::read(a.join("v02.bin")).unwrap()
        );
    }
}
