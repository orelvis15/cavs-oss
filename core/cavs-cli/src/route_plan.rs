//! `cavs route-plan` — pick the best delivery route for one client state
//! (v0.8.0).
//!
//! CAVS is not one patch algorithm; it is a set of routes over the same
//! content-addressed release data. Given what the client actually has —
//! an installed old version, sidecar files the publisher generated, a
//! device profile — the planner measures/estimates every viable route,
//! scores them under the profile's weights and picks one:
//!
//! ```text
//! no-op      already up to date                    0 bytes
//! chunks     warm cache / CDN objects              fresh chunks only
//! hybrid     old install + cold cache              fresh chunks only
//! cavsplan   offline stream patch                  plan bytes, ~40 MiB apply
//! cavspatch  optimized pairwise sidecar            patch bytes, RAM varies
//! bootstrap  fresh install                         whole build compressed
//! full       raw download                          whole build
//! ```
//!
//! Numbers for routes with a real file (`--plan`, `--patch`,
//! `--bootstrap`) are exact; the rest are measured from the builds
//! (chunk diff) or estimated (labelled as such). The auto choice is
//! `min(score)` — with default weights that is simply the smallest
//! network payload whose memory fits the profile.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_chunker::ChunkMode;
use std::path::Path;

const CAVS_MODE: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
    norm: cavs_chunker::NORM_DEFAULT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ClientProfile {
    /// Bytes-first: smallest download wins, memory within 1 GiB.
    Default,
    /// Handhelds/launchers: peak apply RAM capped at 128 MiB.
    LowMemory,
    /// Metered/slow links: network bytes weighted 4×.
    SlowNetwork,
    /// Little free space: temp disk weighted heavily.
    LowDisk,
}

impl ClientProfile {
    fn label(self) -> &'static str {
        match self {
            ClientProfile::Default => "default",
            ClientProfile::LowMemory => "low-memory",
            ClientProfile::SlowNetwork => "slow-network",
            ClientProfile::LowDisk => "low-disk",
        }
    }
    fn ram_budget(self) -> u64 {
        match self {
            ClientProfile::LowMemory => 128 << 20,
            _ => 1 << 30,
        }
    }
    /// (network, apply_ms, temp_disk) weights.
    fn weights(self) -> (f64, f64, f64) {
        match self {
            ClientProfile::Default => (1.0, 100.0, 0.01),
            ClientProfile::LowMemory => (1.0, 100.0, 0.01),
            ClientProfile::SlowNetwork => (4.0, 50.0, 0.01),
            ClientProfile::LowDisk => (1.0, 100.0, 1.0),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RouteCost {
    pub route: String,
    pub network_bytes: u64,
    pub apply_ms_estimate: u64,
    pub peak_ram_bytes: u64,
    pub temp_disk_bytes: u64,
    pub exact: bool,
    pub viable: bool,
    pub notes: String,
    pub score: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct RoutePlanReport {
    pub profile: String,
    pub installed: Option<String>,
    pub new_build: String,
    pub chosen: String,
    pub reason: String,
    pub routes: Vec<RouteCost>,
}

pub struct RoutePlanArgs<'a> {
    pub installed: Option<&'a Path>,
    pub new: &'a Path,
    pub plan: Option<&'a Path>,
    pub patch: Option<&'a Path>,
    pub bootstrap: Option<&'a Path>,
    pub profile: ClientProfile,
    pub json: bool,
}

pub fn route_plan(args: &RoutePlanArgs) -> Result<()> {
    let report = plan(args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "route-plan ({} profile): {}{}",
        report.profile,
        report
            .installed
            .as_deref()
            .map(|i| format!("{i} → "))
            .unwrap_or_default(),
        report.new_build
    );
    for r in &report.routes {
        println!(
            "  {:<12} {:>12}{}  ram {:>8}  {}{}",
            r.route,
            human_bytes(r.network_bytes),
            if r.exact { " " } else { "~" },
            human_bytes(r.peak_ram_bytes),
            if r.viable { "" } else { "[excluded] " },
            r.notes
        );
    }
    println!("\nchosen  : {} — {}", report.chosen, report.reason);
    Ok(())
}

pub fn plan(args: &RoutePlanArgs) -> Result<RoutePlanReport> {
    let new_size = crate::bench_butler::tree_size(args.new)?;
    let mut routes: Vec<RouteCost> = Vec::new();
    let ram_budget = args.profile.ram_budget();

    // ---- no-op ------------------------------------------------------------
    if let Some(installed) = args.installed {
        if installed.is_dir() == args.new.is_dir() && trees_equal(installed, args.new)? {
            let report = RoutePlanReport {
                profile: args.profile.label().into(),
                installed: Some(installed.display().to_string()),
                new_build: args.new.display().to_string(),
                chosen: "no-op".into(),
                reason: "the install already matches the target version".into(),
                routes: vec![RouteCost {
                    route: "no-op".into(),
                    network_bytes: 0,
                    apply_ms_estimate: 0,
                    peak_ram_bytes: 0,
                    temp_disk_bytes: 0,
                    exact: true,
                    viable: true,
                    notes: "already up to date".into(),
                    score: 0.0,
                }],
            };
            return Ok(report);
        }
    }

    // ---- routes that need an old version ----------------------------------
    if let Some(installed) = args.installed {
        // chunk / hybrid: fresh-chunk wire bytes.
        let fresh = fresh_chunk_bytes(installed, args.new)?;
        routes.push(RouteCost {
            route: "chunks".into(),
            network_bytes: fresh,
            apply_ms_estimate: est_apply_ms(new_size),
            peak_ram_bytes: 64 << 20,
            temp_disk_bytes: new_size,
            exact: true,
            viable: true,
            notes: "warm cache or CDN range reads".into(),
            score: 0.0,
        });
        routes.push(RouteCost {
            route: "hybrid".into(),
            network_bytes: fresh,
            apply_ms_estimate: est_apply_ms(new_size),
            peak_ram_bytes: 64 << 20,
            temp_disk_bytes: new_size,
            exact: true,
            viable: true,
            notes: "cold cache + previous install as local source".into(),
            score: 0.0,
        });

        // cavsplan: real file, or built on the spot.
        let (plan_bytes, exact, note) = match args.plan {
            Some(p) => (
                std::fs::metadata(p)?.len(),
                true,
                format!("{}", p.display()),
            ),
            None => {
                let label = installed
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let sig = if installed.is_dir() {
                    cavs_signature::CavsSignature::sign_dir(
                        installed,
                        cavs_signature::DEFAULT_BLOCK_SIZE,
                        &label,
                    )?
                } else {
                    cavs_signature::CavsSignature::sign_file(
                        installed,
                        cavs_signature::DEFAULT_BLOCK_SIZE,
                        &label,
                    )?
                };
                let plan = cavs_plan::build(&sig, args.new, &cavs_plan::BuildOptions::default())?;
                (
                    plan.encode(19).len() as u64,
                    true,
                    "built from the installed version".into(),
                )
            }
        };
        routes.push(RouteCost {
            route: "cavsplan".into(),
            network_bytes: plan_bytes,
            apply_ms_estimate: est_apply_ms(new_size),
            peak_ram_bytes: 40 << 20,
            temp_disk_bytes: new_size / 4,
            exact,
            viable: true,
            notes: note,
            score: 0.0,
        });

        // cavspatch: only when the publisher generated one for this pair.
        match args.patch {
            Some(p) => {
                let bytes = std::fs::read(p)?;
                let (ram, ok_pair) = match crate::patch_v2::PatchV2::decode(&bytes) {
                    Ok(patch) => {
                        let installed_size = crate::bench_butler::tree_size(installed)?;
                        (
                            patch.estimated_apply_peak_bytes(),
                            patch.old_total_size == installed_size,
                        )
                    }
                    Err(_) => (256 << 20, true), // v1 sidecar: conservative
                };
                routes.push(RouteCost {
                    route: "cavspatch".into(),
                    network_bytes: bytes.len() as u64,
                    apply_ms_estimate: est_apply_ms(new_size),
                    peak_ram_bytes: ram,
                    temp_disk_bytes: new_size / 4,
                    exact: true,
                    viable: ram <= ram_budget && ok_pair,
                    notes: if !ok_pair {
                        "sidecar is for a different old version".into()
                    } else if ram > ram_budget {
                        format!(
                            "needs ~{} > {} profile budget",
                            human_bytes(ram),
                            human_bytes(ram_budget)
                        )
                    } else {
                        format!("{}", p.display())
                    },
                    score: 0.0,
                });
            }
            None => routes.push(RouteCost {
                route: "cavspatch".into(),
                network_bytes: u64::MAX,
                apply_ms_estimate: 0,
                peak_ram_bytes: 0,
                temp_disk_bytes: 0,
                exact: false,
                viable: false,
                notes: "no sidecar generated for this pair (hot pairs only)".into(),
                score: 0.0,
            }),
        }
    }

    // ---- fresh-install routes ----------------------------------------------
    let (boot_bytes, boot_exact, boot_note) = match args.bootstrap {
        Some(p) => (
            std::fs::metadata(p)?.len(),
            true,
            format!("{}", p.display()),
        ),
        None => (
            estimate_zstd3(args.new)?,
            false,
            "estimated (zstd-3 of the new build)".into(),
        ),
    };
    routes.push(RouteCost {
        route: "bootstrap".into(),
        network_bytes: boot_bytes,
        apply_ms_estimate: est_apply_ms(new_size),
        peak_ram_bytes: 32 << 20,
        temp_disk_bytes: new_size,
        exact: boot_exact,
        viable: true,
        notes: boot_note,
        score: 0.0,
    });
    routes.push(RouteCost {
        route: "full".into(),
        network_bytes: new_size,
        apply_ms_estimate: 0,
        peak_ram_bytes: 16 << 20,
        temp_disk_bytes: 0,
        exact: true,
        viable: true,
        notes: "raw download, no reuse".into(),
        score: 0.0,
    });

    // ---- score + choose -----------------------------------------------------
    let (wn, wc, wd) = args.profile.weights();
    for r in &mut routes {
        if r.peak_ram_bytes > ram_budget {
            r.viable = false;
            if r.notes.is_empty() {
                r.notes = "over the profile's memory budget".into();
            }
        }
        r.score = if r.viable {
            r.network_bytes as f64 * wn
                + r.apply_ms_estimate as f64 * wc
                + r.temp_disk_bytes as f64 * wd
        } else {
            f64::INFINITY
        };
    }
    let best = routes
        .iter()
        .filter(|r| r.viable)
        .min_by(|a, b| a.score.total_cmp(&b.score))
        .ok_or_else(|| anyhow::anyhow!("no viable route"))?;
    let chosen = best.route.clone();
    let reason = format!(
        "{}{} over the wire, fits the {} profile ({} peak)",
        human_bytes(best.network_bytes),
        if best.exact { "" } else { " (estimated)" },
        args.profile.label(),
        human_bytes(best.peak_ram_bytes),
    );

    Ok(RoutePlanReport {
        profile: args.profile.label().into(),
        installed: args.installed.map(|p| p.display().to_string()),
        new_build: args.new.display().to_string(),
        chosen,
        reason,
        routes,
    })
}

fn trees_equal(a: &Path, b: &Path) -> Result<bool> {
    if a.is_file() && b.is_file() {
        return Ok(std::fs::metadata(a)?.len() == std::fs::metadata(b)?.len()
            && std::fs::read(a)? == std::fs::read(b)?);
    }
    if a.is_dir() && b.is_dir() {
        return crate::bench_butler::trees_identical(a, b);
    }
    Ok(false)
}

/// Wire bytes of the chunk route: fresh chunks (not in the old build),
/// zstd-3 compressed — the same math as `cavs bench routes`.
fn fresh_chunk_bytes(old: &Path, new: &Path) -> Result<u64> {
    let mut old_hashes = std::collections::HashSet::new();
    for (_, bytes) in &files_of(old)? {
        for range in cavs_chunker::split(bytes, CAVS_MODE) {
            old_hashes.insert(cavs_hash::hash_chunk(&bytes[range]));
        }
    }
    let mut update = 0u64;
    let mut seen = std::collections::HashSet::new();
    for (_, bytes) in &files_of(new)? {
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

fn estimate_zstd3(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    for (_, bytes) in &files_of(path)? {
        total += zstd::bulk::compress(bytes, 3)?.len() as u64;
    }
    Ok(total)
}

fn files_of(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    if path.is_file() {
        return Ok(vec![(String::new(), std::fs::read(path)?)]);
    }
    let mut out = Vec::new();
    for rel in crate::compare::walk_sorted(path)? {
        let full = path.join(&rel);
        if full.is_file() {
            out.push((
                rel.to_string_lossy().replace('\\', "/"),
                std::fs::read(&full)?,
            ));
        }
    }
    if out.is_empty() {
        bail!("{} contains no files", path.display());
    }
    Ok(out)
}

/// ~500 MB/s reconstruct estimate; only used to break byte ties.
fn est_apply_ms(bytes: u64) -> u64 {
    bytes / (500 * 1024 * 1024 / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn noop_when_already_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_tree(&a, &[("x.bin", pseudo_random(50_000, 1))]);
        write_tree(&b, &[("x.bin", pseudo_random(50_000, 1))]);
        let report = plan(&RoutePlanArgs {
            installed: Some(&a),
            new: &b,
            plan: None,
            patch: None,
            bootstrap: None,
            profile: ClientProfile::Default,
            json: false,
        })
        .unwrap();
        assert_eq!(report.chosen, "no-op");
    }

    #[test]
    fn fresh_install_prefers_bootstrap_and_update_prefers_delta() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = dir.path().join("v1");
        let v2 = dir.path().join("v2");
        let base = pseudo_random(400_000, 2);
        let mut changed = base.clone();
        changed[100..200].copy_from_slice(&pseudo_random(100, 3));
        write_tree(&v1, &[("x.bin", base)]);
        write_tree(&v2, &[("x.bin", changed)]);

        let fresh = plan(&RoutePlanArgs {
            installed: None,
            new: &v2,
            plan: None,
            patch: None,
            bootstrap: None,
            profile: ClientProfile::Default,
            json: false,
        })
        .unwrap();
        assert!(fresh.chosen == "bootstrap" || fresh.chosen == "full");

        let update = plan(&RoutePlanArgs {
            installed: Some(&v1),
            new: &v2,
            plan: None,
            patch: None,
            bootstrap: None,
            profile: ClientProfile::Default,
            json: false,
        })
        .unwrap();
        assert!(
            update.chosen == "cavsplan" || update.chosen == "chunks" || update.chosen == "hybrid",
            "expected a delta route, got {}",
            update.chosen
        );
        let plan_row = update
            .routes
            .iter()
            .find(|r| r.route == "cavsplan")
            .unwrap();
        assert!(plan_row.network_bytes < 100_000);
    }
}
