//! `cavs plan-update` (v0.9.0): choose the best delivery route for a
//! client state under a *policy* — more than smallest download. Policies
//! weight network, apply CPU, peak RAM, temporary disk and old-install
//! reads; the planner scores every available route, excludes missing
//! ones, and explains the choice.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_chunker::ChunkMode;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

const CAVS_MODE: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
};

/// Score weights. Units: bytes are scored in MiB so weights stay
/// human-sized.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Policy {
    pub name: &'static str,
    pub network: f64,
    pub apply_ms: f64,
    pub ram_mb: f64,
    pub temp_disk: f64,
    pub disk_read: f64,
    /// Weight on diff/build time (developer-side cost).
    pub build_ms: f64,
}

pub const POLICIES: &[Policy] = &[
    Policy {
        name: "network_min",
        network: 100.0,
        apply_ms: 0.01,
        ram_mb: 0.01,
        temp_disk: 0.001,
        disk_read: 0.001,
        build_ms: 0.0,
    },
    Policy {
        name: "cpu_min",
        network: 1.0,
        apply_ms: 10.0,
        ram_mb: 0.01,
        temp_disk: 0.001,
        disk_read: 0.001,
        build_ms: 0.0,
    },
    Policy {
        name: "ram_min",
        network: 1.0,
        apply_ms: 0.01,
        ram_mb: 100.0,
        temp_disk: 0.001,
        disk_read: 0.001,
        build_ms: 0.0,
    },
    Policy {
        name: "disk_io_min",
        network: 1.0,
        apply_ms: 0.01,
        ram_mb: 0.01,
        temp_disk: 10.0,
        disk_read: 10.0,
        build_ms: 0.0,
    },
    Policy {
        name: "balanced",
        network: 10.0,
        apply_ms: 0.05,
        ram_mb: 0.05,
        temp_disk: 0.01,
        disk_read: 0.01,
        build_ms: 0.0,
    },
    Policy {
        name: "hdd_friendly",
        network: 2.0,
        apply_ms: 0.05,
        ram_mb: 0.1,
        temp_disk: 20.0,
        disk_read: 20.0,
        build_ms: 0.0,
    },
    Policy {
        name: "developer_fast",
        network: 1.0,
        apply_ms: 0.05,
        ram_mb: 0.01,
        temp_disk: 0.01,
        disk_read: 0.01,
        build_ms: 10.0,
    },
];

pub fn policy(name: &str) -> Result<Policy> {
    POLICIES
        .iter()
        .find(|p| p.name == name)
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown policy '{name}' (available: {})",
                POLICIES
                    .iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Client state flags parsed from `--client-state a,b,c`.
#[derive(Default, Debug, Clone)]
pub struct ClientState {
    pub warm_cache: bool,
    pub has_previous_install: bool,
    pub low_ram: bool,
    pub low_disk: bool,
    pub slow_hdd: bool,
}

impl ClientState {
    pub fn parse(s: &str) -> Result<ClientState> {
        let mut st = ClientState::default();
        for flag in s.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            match flag {
                "warm-cache" => st.warm_cache = true,
                "cold-cache" => st.warm_cache = false,
                "has-previous-install" | "previous-install" => st.has_previous_install = true,
                "cold-install" | "fresh-install" | "no-previous-install" => {
                    st.has_previous_install = false
                }
                "low-ram" => st.low_ram = true,
                "low-disk" | "limited-disk" => st.low_disk = true,
                "slow-hdd" => st.slow_hdd = true,
                "fast-nvme" => st.slow_hdd = false,
                other => bail!(
                    "unknown client state '{other}' (warm-cache, cold-cache, \
                     has-previous-install, cold-install, low-ram, low-disk, slow-hdd, fast-nvme)"
                ),
            }
        }
        Ok(st)
    }
    pub fn ram_budget(&self) -> u64 {
        if self.low_ram {
            128 << 20
        } else {
            1 << 30
        }
    }
}

/// Extra score per patch-apply step beyond the first: a chain of
/// sequential patches multiplies the failure surface (every intermediate
/// patch must exist, download and apply cleanly), so the planner never
/// picks a long chain just because it saves a few KiB.
pub const STEP_RISK_WEIGHT: f64 = 25.0;

#[derive(Serialize, Clone)]
pub struct ScoredRoute {
    pub route: String,
    pub available: bool,
    pub network_bytes: u64,
    pub apply_ms: u64,
    pub build_ms: u64,
    pub peak_ram_bytes: u64,
    pub temp_disk_bytes: u64,
    pub disk_read_bytes: u64,
    /// Sequential patch applications this route needs (1 for direct
    /// routes; >1 for adjacent/ladder patch chains fed from a graph).
    pub patch_steps: usize,
    pub exact: bool,
    pub score: f64,
    pub notes: String,
}

#[derive(Serialize)]
pub struct PlanUpdateReport {
    pub from: Option<String>,
    pub to: String,
    pub policy: String,
    pub client_state: String,
    pub chosen: String,
    pub reason: String,
    pub routes: Vec<ScoredRoute>,
}

pub struct PlanUpdateArgs<'a> {
    /// Installed/old version (path), if the client has one.
    pub from: Option<&'a Path>,
    /// Target version (path).
    pub to: &'a Path,
    /// Pre-generated exact artifacts for this pair, when the publisher
    /// made them.
    pub plan_file: Option<&'a Path>,
    pub patch_file: Option<&'a Path>,
    pub bootstrap_file: Option<&'a Path>,
    pub client_state: &'a str,
    pub policy: &'a str,
    pub json: bool,
}

fn files_of(path: &Path) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for (_, abs) in cavs_analyzer::walk::walk(path)? {
        out.push(std::fs::read(&abs)?);
    }
    if out.is_empty() {
        bail!("{} contains no files", path.display());
    }
    Ok(out)
}

fn fresh_chunk_bytes(old: &Path, new: &Path) -> Result<u64> {
    let mut old_hashes = HashSet::new();
    for bytes in &files_of(old)? {
        for range in cavs_chunker::split(bytes, CAVS_MODE) {
            old_hashes.insert(cavs_hash::hash_chunk(&bytes[range]));
        }
    }
    let mut update = 0u64;
    let mut seen = HashSet::new();
    for bytes in &files_of(new)? {
        for range in cavs_chunker::split(bytes, CAVS_MODE) {
            let chunk = &bytes[range];
            let hash = cavs_hash::hash_chunk(chunk);
            if !old_hashes.contains(&hash) && seen.insert(hash) {
                update += zstd::bulk::compress(chunk, 3)?.len() as u64;
            }
        }
    }
    Ok(update)
}

fn tree_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    for (_, abs) in cavs_analyzer::walk::walk(path)? {
        total += std::fs::metadata(&abs)?.len();
    }
    Ok(total)
}

/// ~500 MB/s reconstruct estimate; used for score tie-breaking only.
fn est_apply_ms(bytes: u64) -> u64 {
    bytes / (500 * 1024 * 1024 / 1000)
}

pub fn collect_routes(args: &PlanUpdateArgs, state: &ClientState) -> Result<Vec<ScoredRoute>> {
    let new_size = tree_size(args.to)?;
    let mut routes = Vec::new();
    let unavailable = |route: &str, why: &str| ScoredRoute {
        route: route.into(),
        available: false,
        network_bytes: u64::MAX,
        apply_ms: 0,
        build_ms: 0,
        peak_ram_bytes: 0,
        temp_disk_bytes: 0,
        disk_read_bytes: 0,
        patch_steps: 1,
        exact: false,
        score: f64::INFINITY,
        notes: why.into(),
    };

    // -- full & bootstrap: always available ---------------------------------
    routes.push(ScoredRoute {
        route: "full download".into(),
        available: true,
        network_bytes: new_size,
        apply_ms: 0,
        build_ms: 0,
        peak_ram_bytes: 16 << 20,
        temp_disk_bytes: 0,
        disk_read_bytes: 0,
        patch_steps: 1,
        exact: true,
        score: 0.0,
        notes: "raw download, no reuse".into(),
    });
    let (boot_bytes, boot_exact, boot_note) = match args.bootstrap_file {
        Some(p) => (std::fs::metadata(p)?.len(), true, p.display().to_string()),
        None => {
            let t0 = std::time::Instant::now();
            let mut c = 0u64;
            for bytes in &files_of(args.to)? {
                c += zstd::bulk::compress(bytes, 3)?.len() as u64;
            }
            let _ = t0;
            (c, false, "estimated (zstd-3 of the target)".into())
        }
    };
    routes.push(ScoredRoute {
        route: "bootstrap".into(),
        available: true,
        network_bytes: boot_bytes,
        apply_ms: est_apply_ms(new_size),
        build_ms: 0,
        peak_ram_bytes: 32 << 20,
        temp_disk_bytes: new_size,
        disk_read_bytes: 0,
        patch_steps: 1,
        exact: boot_exact,
        score: 0.0,
        notes: boot_note,
    });

    // -- routes that need a previous install --------------------------------
    match (args.from, state.has_previous_install || args.from.is_some()) {
        (Some(old), true) => {
            let old_size = tree_size(old)?;
            let fresh = fresh_chunk_bytes(old, args.to)?;
            if state.warm_cache {
                routes.push(ScoredRoute {
                    route: "chunk route (warm cache)".into(),
                    available: true,
                    network_bytes: fresh,
                    apply_ms: est_apply_ms(new_size),
                    build_ms: 0,
                    peak_ram_bytes: 64 << 20,
                    temp_disk_bytes: new_size,
                    disk_read_bytes: 0,
                    patch_steps: 1,
                    exact: true,
                    score: 0.0,
                    notes: "cache already holds old chunks".into(),
                });
            } else {
                routes.push(unavailable(
                    "chunk route (warm cache)",
                    "client cache is cold (state: cold-cache)",
                ));
            }
            routes.push(ScoredRoute {
                route: "hybrid previous-artifact".into(),
                available: true,
                network_bytes: fresh,
                apply_ms: est_apply_ms(new_size),
                build_ms: 0,
                peak_ram_bytes: 64 << 20,
                temp_disk_bytes: new_size,
                disk_read_bytes: old_size,
                patch_steps: 1,
                exact: true,
                score: 0.0,
                notes: "cold cache + previous install as local source".into(),
            });

            let (plan_bytes, plan_exact, plan_note, plan_build_ms) = match args.plan_file {
                Some(p) => (
                    std::fs::metadata(p)?.len(),
                    true,
                    p.display().to_string(),
                    0,
                ),
                None => {
                    let t0 = std::time::Instant::now();
                    let label = old
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let sig = if old.is_dir() {
                        cavs_signature::CavsSignature::sign_dir(
                            old,
                            cavs_signature::DEFAULT_BLOCK_SIZE,
                            &label,
                        )?
                    } else {
                        cavs_signature::CavsSignature::sign_file(
                            old,
                            cavs_signature::DEFAULT_BLOCK_SIZE,
                            &label,
                        )?
                    };
                    let plan =
                        cavs_plan::build(&sig, args.to, &cavs_plan::BuildOptions::default())?;
                    (
                        plan.encode(19).len() as u64,
                        true,
                        "built from the installed version".into(),
                        t0.elapsed().as_millis() as u64,
                    )
                }
            };
            routes.push(ScoredRoute {
                route: ".cavsplan".into(),
                available: true,
                network_bytes: plan_bytes,
                apply_ms: est_apply_ms(new_size),
                build_ms: plan_build_ms,
                peak_ram_bytes: 40 << 20,
                temp_disk_bytes: new_size / 4,
                disk_read_bytes: old_size,
                patch_steps: 1,
                exact: plan_exact,
                score: 0.0,
                notes: plan_note,
            });

            match args.patch_file {
                Some(p) => {
                    let bytes = std::fs::read(p)?;
                    let ram = crate::patch_v2::PatchV2::decode(&bytes)
                        .map(|patch| patch.estimated_apply_peak_bytes())
                        .unwrap_or(256 << 20);
                    routes.push(ScoredRoute {
                        route: "optimized sidecar (.cavspatch)".into(),
                        available: true,
                        network_bytes: bytes.len() as u64,
                        apply_ms: est_apply_ms(new_size),
                        build_ms: 0,
                        peak_ram_bytes: ram,
                        temp_disk_bytes: new_size / 4,
                        disk_read_bytes: old_size,
                        patch_steps: 1,
                        exact: true,
                        score: 0.0,
                        notes: p.display().to_string(),
                    });
                }
                None => routes.push(unavailable(
                    "optimized sidecar (.cavspatch)",
                    "no sidecar generated for this pair (hot pairs only)",
                )),
            }

            // External pairwise routes: never chosen unless measured.
            routes.push(unavailable(
                "butler offline",
                "not measured here — run `cavs bench routes --butler-bin butler`",
            ));
            routes.push(unavailable(
                "bsdiff pairwise proxy",
                "not measured here — run `cavs bench pairwise-proxy` (high apply RAM)",
            ));
            routes.push(unavailable(
                "xdelta3 pairwise proxy",
                "not measured here — run `cavs bench pairwise-proxy`",
            ));
        }
        _ => {
            routes.push(unavailable(
                "chunk route (warm cache)",
                "no previous install",
            ));
            routes.push(unavailable(
                "hybrid previous-artifact",
                "no previous install",
            ));
            routes.push(unavailable(".cavsplan", "no previous install"));
            routes.push(unavailable(
                "optimized sidecar (.cavspatch)",
                "no previous install",
            ));
        }
    }
    Ok(routes)
}

pub fn score_and_choose(
    routes: &mut [ScoredRoute],
    pol: &Policy,
    state: &ClientState,
) -> Result<(String, String)> {
    let ram_budget = state.ram_budget();
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    for r in routes.iter_mut() {
        if !r.available {
            r.score = f64::INFINITY;
            continue;
        }
        if r.peak_ram_bytes > ram_budget {
            r.available = false;
            r.notes = format!(
                "needs ~{} peak RAM > {} budget",
                human_bytes(r.peak_ram_bytes),
                human_bytes(ram_budget)
            );
            r.score = f64::INFINITY;
            continue;
        }
        let seek_penalty = if state.slow_hdd {
            // full-file copies hurt on HDDs: penalize temp+read heavily.
            (mib(r.temp_disk_bytes) + mib(r.disk_read_bytes)) * 5.0
        } else {
            0.0
        };
        // Little free space: temporary disk is the scarce resource.
        let temp_weight = if state.low_disk {
            pol.temp_disk.max(1.0) * 20.0
        } else {
            pol.temp_disk
        };
        // Chains of sequential patch applies carry recovery risk beyond
        // their raw byte/CPU cost (see docs/ROUTE_PLANNER.md).
        let risk_penalty = r.patch_steps.saturating_sub(1) as f64 * STEP_RISK_WEIGHT;
        r.score = mib(r.network_bytes) * pol.network
            + r.apply_ms as f64 * pol.apply_ms
            + mib(r.peak_ram_bytes) * pol.ram_mb
            + mib(r.temp_disk_bytes) * temp_weight
            + mib(r.disk_read_bytes) * pol.disk_read
            + r.build_ms as f64 * pol.build_ms
            + seek_penalty
            + risk_penalty;
    }
    let best = routes
        .iter()
        .filter(|r| r.available)
        .min_by(|a, b| a.score.total_cmp(&b.score))
        .ok_or_else(|| anyhow::anyhow!("CAVS-E-ROUTE-NOT-AVAILABLE: no viable route"))?;
    let reason = format!(
        "{}{} over the wire · ~{} peak RAM · {} temp disk · policy {}",
        human_bytes(best.network_bytes),
        if best.exact { "" } else { " (estimated)" },
        human_bytes(best.peak_ram_bytes),
        human_bytes(best.temp_disk_bytes),
        pol.name,
    );
    Ok((best.route.clone(), reason))
}

pub fn plan_update(args: &PlanUpdateArgs) -> Result<()> {
    let state = ClientState::parse(args.client_state)?;
    let pol = policy(args.policy)?;
    let mut routes = collect_routes(args, &state)?;
    let (chosen, reason) = score_and_choose(&mut routes, &pol, &state)?;
    routes.sort_by(|a, b| a.score.total_cmp(&b.score));
    let report = PlanUpdateReport {
        from: args.from.map(|p| p.display().to_string()),
        to: args.to.display().to_string(),
        policy: pol.name.into(),
        client_state: args.client_state.into(),
        chosen,
        reason,
        routes,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "plan-update ({} policy, state: {}):",
        report.policy,
        if report.client_state.is_empty() {
            "default"
        } else {
            &report.client_state
        }
    );
    for r in &report.routes {
        if r.available {
            println!(
                "  {:<32} {:>12}{}  ram {:>9}  temp {:>10}  score {:.0}",
                r.route,
                human_bytes(r.network_bytes),
                if r.exact { " " } else { "~" },
                human_bytes(r.peak_ram_bytes),
                human_bytes(r.temp_disk_bytes),
                r.score,
            );
        } else {
            println!("  {:<32} [unavailable] {}", r.route, r.notes);
        }
    }
    println!("\nchosen  : {} — {}", report.chosen, report.reason);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_tree(root: &Path, files: &[(&str, Vec<u8>)]) {
        for (rel, bytes) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    fn pair(dir: &Path) -> (PathBuf, PathBuf) {
        let v1 = dir.join("v1");
        let v2 = dir.join("v2");
        let base = pseudo_random(600_000, 2);
        let mut changed = base.clone();
        changed[1000..1100].copy_from_slice(&pseudo_random(100, 3));
        write_tree(&v1, &[("x.bin", base)]);
        write_tree(&v2, &[("x.bin", changed)]);
        (v1, v2)
    }

    #[test]
    fn every_policy_parses() {
        for p in POLICIES {
            assert!(policy(p.name).is_ok());
        }
        assert!(policy("nope").is_err());
    }

    #[test]
    fn update_prefers_delta_and_fresh_prefers_bootstrap() {
        let dir = tempfile::tempdir().unwrap();
        let (v1, v2) = pair(dir.path());

        let state = ClientState::parse("has-previous-install").unwrap();
        let mut routes = collect_routes(
            &PlanUpdateArgs {
                from: Some(&v1),
                to: &v2,
                plan_file: None,
                patch_file: None,
                bootstrap_file: None,
                client_state: "has-previous-install",
                policy: "balanced",
                json: false,
            },
            &state,
        )
        .unwrap();
        let (chosen, _) =
            score_and_choose(&mut routes, &policy("balanced").unwrap(), &state).unwrap();
        assert!(
            chosen == ".cavsplan" || chosen.starts_with("hybrid"),
            "chosen {chosen}"
        );

        let state = ClientState::parse("cold-install").unwrap();
        let mut routes = collect_routes(
            &PlanUpdateArgs {
                from: None,
                to: &v2,
                plan_file: None,
                patch_file: None,
                bootstrap_file: None,
                client_state: "cold-install",
                policy: "balanced",
                json: false,
            },
            &state,
        )
        .unwrap();
        let (chosen, _) =
            score_and_choose(&mut routes, &policy("balanced").unwrap(), &state).unwrap();
        assert!(
            chosen == "bootstrap" || chosen == "full download",
            "chosen {chosen}"
        );
    }

    #[test]
    fn missing_routes_are_never_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let (v1, v2) = pair(dir.path());
        let state = ClientState::parse("").unwrap();
        let mut routes = collect_routes(
            &PlanUpdateArgs {
                from: Some(&v1),
                to: &v2,
                plan_file: None,
                patch_file: None,
                bootstrap_file: None,
                client_state: "",
                policy: "network_min",
                json: false,
            },
            &state,
        )
        .unwrap();
        let (chosen, _) =
            score_and_choose(&mut routes, &policy("network_min").unwrap(), &state).unwrap();
        // butler/bsdiff/xdelta/sidecar are all unavailable.
        assert!(!chosen.contains("butler") && !chosen.contains("bsdiff"));
        for r in routes.iter().filter(|r| !r.available) {
            assert!(r.score.is_infinite());
        }
    }

    #[test]
    fn warm_cache_unlocks_the_chunk_route() {
        let dir = tempfile::tempdir().unwrap();
        let (v1, v2) = pair(dir.path());
        for (states, expect_chunk) in [
            ("warm-cache,has-previous-install", true),
            ("has-previous-install", false),
        ] {
            let state = ClientState::parse(states).unwrap();
            let routes = collect_routes(
                &PlanUpdateArgs {
                    from: Some(&v1),
                    to: &v2,
                    plan_file: None,
                    patch_file: None,
                    bootstrap_file: None,
                    client_state: states,
                    policy: "balanced",
                    json: false,
                },
                &state,
            )
            .unwrap();
            let chunk = routes
                .iter()
                .find(|r| r.route.starts_with("chunk route"))
                .unwrap();
            assert_eq!(chunk.available, expect_chunk, "state {states}");
        }
    }

    #[test]
    fn client_state_parsing_rejects_unknown() {
        assert!(ClientState::parse("warm-cache,low-ram").is_ok());
        assert!(ClientState::parse("hyperspace").is_err());
        assert_eq!(
            ClientState::parse("low-ram").unwrap().ram_budget(),
            128 << 20
        );
    }
}
