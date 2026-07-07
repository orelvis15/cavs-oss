//! `cavs bench steampipe-cases` (v0.9.0): the SteamPipe-style model and
//! pack-pathology benchmark (plan benchmarks A, B, C and G).
//!
//! Generates deterministic pack layouts that exercise the classic update
//! failure modes — localized edits, shifts, shuffles, distributed TOC
//! churn, global vs per-asset compression, new-content-as-new-pack, a
//! directory build, and synthetic Godot PCKs — and measures each
//! old→new transition under:
//!
//! - the SteamPipe-style fixed 1 MiB model (changed chunks, est. bytes);
//! - CAVS FastCDC reuse;
//! - a real `.cavsplan` (built and sized, not estimated);
//! - butler / bsdiff / xdelta3 when those tools are installed (skipped
//!   with a note otherwise).

use crate::report::human_bytes;
use anyhow::Result;
use cavs_analyzer::steampipe::{estimate, ModelConfig};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// xorshift64* PRNG (same family as synth.rs) for deterministic data.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for chunk in out.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&v[..n]);
        }
        out
    }
}

const ASSET: usize = 1 << 20; // 1 MiB per asset

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// One generated case: old/new directories plus a description.
struct Case {
    name: &'static str,
    description: &'static str,
    old: PathBuf,
    new: PathBuf,
}

fn generate_cases(root: &Path, assets_count: usize, seed: u64) -> Result<Vec<Case>> {
    let mut rng = Rng::new(seed);
    // Odd sizes on purpose: real assets are never exactly chunk-aligned,
    // and perfectly 1 MiB-aligned assets would make reordering look free
    // under the fixed model.
    let assets: Vec<Vec<u8>> = (0..assets_count)
        .map(|i| rng.bytes(ASSET + (i * 4931 + 12_345) % 60_000))
        .collect();
    // A "compressible" variant: each asset is a repeated 4 KiB pattern,
    // so compression actually shrinks it (pure noise would not).
    let compressible: Vec<Vec<u8>> = (0..assets_count)
        .map(|_| {
            let block = rng.bytes(4096);
            block.iter().cycle().take(ASSET).copied().collect()
        })
        .collect();
    let mut edited = assets.clone();
    {
        // one 64 KiB region inside asset 7
        let patch = rng.bytes(64 * 1024);
        edited[7][100_000..100_000 + patch.len()].copy_from_slice(&patch);
    }
    let mut edited_medium = assets.clone();
    for i in [3usize, 9, 15, 21] {
        let patch = rng.bytes(200 * 1024);
        edited_medium[i % assets_count][50_000..50_000 + patch.len()].copy_from_slice(&patch);
    }
    let mut compressible_edited = compressible.clone();
    {
        let patch = rng.bytes(64 * 1024);
        compressible_edited[5][10_000..10_000 + patch.len()].copy_from_slice(&patch);
    }

    let concat = |xs: &[Vec<u8>]| xs.concat();
    let mut cases = Vec::new();
    let case = |name: &'static str,
                description: &'static str,
                old_files: Vec<(&str, Vec<u8>)>,
                new_files: Vec<(&str, Vec<u8>)>|
     -> Result<Case> {
        let old = root.join(name).join("old");
        let new = root.join(name).join("new");
        for (rel, bytes) in &old_files {
            write(&old.join(rel), bytes)?;
        }
        for (rel, bytes) in &new_files {
            write(&new.join(rel), bytes)?;
        }
        Ok(Case {
            name,
            description,
            old,
            new,
        })
    };

    // A1/A2 — localized changes.
    cases.push(case(
        "pack-localized-small",
        "one 64 KiB edit inside a big pack",
        vec![("world.pak", concat(&assets))],
        vec![("world.pak", concat(&edited))],
    )?);
    cases.push(case(
        "pack-localized-medium",
        "four 200 KiB edits spread over the pack",
        vec![("world.pak", concat(&assets))],
        vec![("world.pak", concat(&edited_medium))],
    )?);

    // A3 — shifted: 4 KiB grows at the front, everything after slides.
    let mut shifted = rng.bytes(4096);
    shifted.extend(concat(&assets));
    cases.push(case(
        "pack-shifted",
        "4 KiB inserted at the front; every byte after shifts",
        vec![("world.pak", concat(&assets))],
        vec![("world.pak", shifted)],
    )?);

    // A4 — shuffled: same assets, rotated order.
    let rotated: Vec<u8> = assets
        .iter()
        .skip(1)
        .chain(assets.iter().take(1))
        .flat_map(|a| a.iter().copied())
        .collect();
    cases.push(case(
        "pack-shuffled",
        "same assets, new order",
        vec![("world.pak", concat(&assets))],
        vec![("world.pak", rotated)],
    )?);

    // A5 — distributed TOC churn: a per-asset metadata header (build id
    // + offset) in front of every asset. A new build bumps the id in
    // every header, so tiny edits dirty every 1 MiB window even though
    // asset bytes barely change. Asset sizes stay constant to isolate
    // the header churn from shift effects.
    let toc_pack = |xs: &[Vec<u8>], build_id: u64| {
        let mut out = Vec::new();
        let mut offset = 0u64;
        for a in xs {
            out.extend_from_slice(&build_id.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(a);
            offset += 16 + a.len() as u64;
        }
        out
    };
    cases.push(case(
        "pack-toc-distributed",
        "per-asset headers rewritten every build (build id); one 64 KiB real edit",
        vec![("world.pak", toc_pack(&assets, 1))],
        vec![("world.pak", toc_pack(&edited, 2))],
    )?);

    // The same edit with the TOC at the end only (recommended layout).
    let toc_end_pack = |xs: &[Vec<u8>], build_id: u64| {
        let mut out = concat(xs);
        out.extend_from_slice(&build_id.to_le_bytes());
        for a in xs {
            out.extend_from_slice(&(a.len() as u64).to_le_bytes());
        }
        out
    };
    cases.push(case(
        "pack-toc-end",
        "same edit and build id bump with the TOC at the end only",
        vec![("world.pak", toc_end_pack(&assets, 1))],
        vec![("world.pak", toc_end_pack(&edited, 2))],
    )?);

    // A6/A7, C3/C4 — global vs per-asset compression.
    cases.push(case(
        "pack-global-compressed",
        "whole pack zstd-3 as one stream; one 64 KiB source edit",
        vec![(
            "world.pak",
            zstd::bulk::compress(&concat(&compressible), 3)?,
        )],
        vec![(
            "world.pak",
            zstd::bulk::compress(&concat(&compressible_edited), 3)?,
        )],
    )?);
    // Per-asset compression with slot padding (what alignment-aware
    // engines do): each compressed asset sits in a fixed 128 KiB slot,
    // so an edit stays inside its own slot.
    let per_asset = |xs: &[Vec<u8>]| -> Result<Vec<u8>> {
        const SLOT: usize = 128 * 1024;
        let mut out = Vec::new();
        for a in xs {
            let mut c = zstd::bulk::compress(a, 3)?;
            c.resize(c.len().div_ceil(SLOT) * SLOT, 0);
            out.extend(c);
        }
        Ok(out)
    };
    cases.push(case(
        "pack-per-asset-compressed",
        "each asset compressed into a padded 128 KiB slot; same 64 KiB source edit",
        vec![("world.pak", per_asset(&compressible)?)],
        vec![("world.pak", per_asset(&compressible_edited)?)],
    )?);

    // A8 — new content shipped as a new pack (old pack untouched).
    let new_content: Vec<Vec<u8>> = (0..4).map(|_| rng.bytes(ASSET)).collect();
    cases.push(case(
        "new-content-new-pack",
        "4 new assets ship as a new pack; the old pack stays identical",
        vec![("world.pak", concat(&assets))],
        vec![
            ("world.pak", concat(&assets)),
            ("world_dlc.pak", concat(&new_content)),
        ],
    )?);

    // C1 — the same content as a directory build (per-asset files).
    let dir_files = |xs: &[Vec<u8>]| -> Vec<(String, Vec<u8>)> {
        xs.iter()
            .enumerate()
            .map(|(i, a)| (format!("assets/asset_{i:03}.bin"), a.clone()))
            .collect()
    };
    {
        let old = root.join("directory-build").join("old");
        let new = root.join("directory-build").join("new");
        for (rel, bytes) in dir_files(&assets) {
            write(&old.join(&rel), &bytes)?;
        }
        for (rel, bytes) in dir_files(&edited) {
            write(&new.join(&rel), &bytes)?;
        }
        cases.push(Case {
            name: "directory-build",
            description: "same assets as individual files; one 64 KiB edit",
            old,
            new,
        });
    }

    // G — synthetic Godot PCKs: localized resource edit vs shifted pack.
    {
        let hero = rng.bytes(512 * 1024);
        let mut hero2 = hero.clone();
        let patch = rng.bytes(32 * 1024);
        hero2[100_000..100_000 + patch.len()].copy_from_slice(&patch);
        let level = rng.bytes(2 << 20);
        let files_old = vec![
            ("res://textures/hero.png", hero.as_slice()),
            ("res://levels/level01.scn", level.as_slice()),
        ];
        let files_new = vec![
            ("res://textures/hero.png", hero2.as_slice()),
            ("res://levels/level01.scn", level.as_slice()),
        ];
        let old = root.join("godot-pck-localized").join("old");
        let new = root.join("godot-pck-localized").join("new");
        write(
            &old.join("game.pck"),
            &crate::analyze_packs::godot_pck::synth(2, &files_old),
        )?;
        write(
            &new.join("game.pck"),
            &crate::analyze_packs::godot_pck::synth(2, &files_new),
        )?;
        cases.push(Case {
            name: "godot-pck-localized",
            description: "Godot PCK with one edited resource",
            old,
            new,
        });

        // Many resources added in front (offsets of old resources shift).
        let extra = rng.bytes(1 << 20);
        let files_grown = vec![
            ("res://levels/level02.scn", extra.as_slice()),
            ("res://textures/hero.png", hero.as_slice()),
            ("res://levels/level01.scn", level.as_slice()),
        ];
        let old = root.join("godot-pck-shifted").join("old");
        let new = root.join("godot-pck-shifted").join("new");
        write(
            &old.join("game.pck"),
            &crate::analyze_packs::godot_pck::synth(2, &files_old),
        )?;
        write(
            &new.join("game.pck"),
            &crate::analyze_packs::godot_pck::synth(2, &files_grown),
        )?;
        cases.push(Case {
            name: "godot-pck-shifted",
            description: "Godot PCK with a new resource packed first (offset shift)",
            old,
            new,
        });
    }

    Ok(cases)
}

#[derive(Serialize)]
struct CaseResult {
    case: String,
    description: String,
    old_bytes: u64,
    new_bytes: u64,
    steampipe_changed_chunks: u64,
    steampipe_total_chunks: u64,
    steampipe_estimate_bytes: u64,
    fixed_reuse_pct: f64,
    cdc_reuse_pct: f64,
    cavsplan_bytes: u64,
    cavsplan_build_ms: u64,
    butler_bytes: Option<u64>,
    bsdiff_bytes: Option<u64>,
    xdelta3_bytes: Option<u64>,
    diagnosis: String,
}

#[derive(Serialize)]
struct CasesReport {
    seed: u64,
    assets_per_pack: usize,
    results: Vec<CaseResult>,
    skipped: Vec<String>,
    note: String,
}

pub struct CasesArgs<'a> {
    pub out: &'a Path,
    pub assets: usize,
    pub seed: u64,
    pub butler_bin: Option<&'a str>,
    pub include_pairwise: bool,
    pub keep_datasets: bool,
}

pub fn bench(args: &CasesArgs) -> Result<()> {
    std::fs::create_dir_all(args.out)?;
    let data_root = args.out.join("datasets");
    let cases = generate_cases(&data_root, args.assets, args.seed)?;

    let mut results = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut butler_done = false;

    for case in &cases {
        eprintln!("[steampipe-cases] {} …", case.name);
        let est = estimate(&case.old, &case.new, &ModelConfig::default(), &|_| true)?;

        // Real .cavsplan for the pair.
        let t0 = std::time::Instant::now();
        let sig = if case.old.is_dir() {
            cavs_signature::CavsSignature::sign_dir(
                &case.old,
                cavs_signature::DEFAULT_BLOCK_SIZE,
                case.name,
            )?
        } else {
            cavs_signature::CavsSignature::sign_file(
                &case.old,
                cavs_signature::DEFAULT_BLOCK_SIZE,
                case.name,
            )?
        };
        let plan = cavs_plan::build(&sig, &case.new, &cavs_plan::BuildOptions::default())?;
        let cavsplan_bytes = plan.encode(19).len() as u64;
        let cavsplan_build_ms = t0.elapsed().as_millis() as u64;

        // CDC (content similarity) from the analyzer's contrast model.
        let analysis = cavs_analyzer::compare::analyze(
            &case.old,
            &case.new,
            cavs_analyzer::Engine::Auto,
            &cavs_analyzer::detect::Thresholds::default(),
            &|_| true,
        )?;
        let diagnosis = analysis
            .findings
            .iter()
            .filter(|f| f.kind != "engine_hint")
            .map(|f| f.kind.clone())
            .collect::<Vec<_>>()
            .join(", ");

        // External tools, when available.
        let (mut butler_bytes, mut bsdiff_bytes, mut xdelta3_bytes) = (None, None, None);
        if let Some(butler) = args.butler_bin {
            let work = args.out.join("butler-work").join(case.name);
            match crate::bench_butler::run(&case.old, &case.new, butler, &work) {
                Ok(r) => butler_bytes = Some(r.patch_pwr_bytes),
                Err(e) => {
                    if !butler_done {
                        skipped.push(format!("butler: {e}"));
                        butler_done = true;
                    }
                }
            }
        }
        if args.include_pairwise {
            match crate::bench_pairwise::run(&case.old, &case.new, "bsdiff,xdelta3", "zstd-19") {
                Ok(p) => {
                    for r in &p.results {
                        if r.method.contains("bsdiff") {
                            bsdiff_bytes = Some(r.patch_bytes_compressed);
                        } else if r.method.contains("xdelta3") {
                            xdelta3_bytes = Some(r.patch_bytes_compressed);
                        }
                    }
                    for n in p.notes {
                        if !skipped.contains(&n) {
                            skipped.push(n);
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("pairwise: {e}");
                    if !skipped.contains(&msg) {
                        skipped.push(msg);
                    }
                }
            }
        }

        let reuse = |part: u64, whole: u64| {
            if whole == 0 {
                100.0
            } else {
                (1.0 - part as f64 / whole as f64) * 100.0
            }
        };
        results.push(CaseResult {
            case: case.name.into(),
            description: case.description.into(),
            old_bytes: est.old_size_bytes,
            new_bytes: est.new_size_bytes,
            steampipe_changed_chunks: est.new_or_changed_chunks,
            steampipe_total_chunks: est.total_chunks_new,
            steampipe_estimate_bytes: est.estimated_download_compressed,
            fixed_reuse_pct: reuse(est.estimated_download_raw, est.new_size_bytes),
            cdc_reuse_pct: analysis.cdc_reuse_ratio * 100.0,
            cavsplan_bytes,
            cavsplan_build_ms,
            butler_bytes,
            bsdiff_bytes,
            xdelta3_bytes,
            diagnosis: if diagnosis.is_empty() {
                "localized / OK".into()
            } else {
                diagnosis
            },
        });
    }

    let report = CasesReport {
        seed: args.seed,
        assets_per_pack: args.assets,
        results,
        skipped,
        note: cavs_analyzer::ESTIMATE_NOTE.into(),
    };

    print_table(&report);
    std::fs::write(
        args.out.join("steampipe-cases.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(args.out.join("steampipe-cases.md"), markdown(&report))?;
    if !args.keep_datasets {
        let _ = std::fs::remove_dir_all(&data_root);
        let _ = std::fs::remove_dir_all(args.out.join("butler-work"));
    }
    println!(
        "results : {}/steampipe-cases.md + steampipe-cases.json",
        args.out.display()
    );
    Ok(())
}

fn print_table(r: &CasesReport) {
    for res in &r.results {
        println!(
            "  {:<26} steampipe {:>10}  cavsplan {:>10}  fixed {:>5.1}%  cdc {:>5.1}%  {}",
            res.case,
            human_bytes(res.steampipe_estimate_bytes),
            human_bytes(res.cavsplan_bytes),
            res.fixed_reuse_pct,
            res.cdc_reuse_pct,
            res.diagnosis,
        );
    }
    for s in &r.skipped {
        println!("  skipped: {s}");
    }
}

fn markdown(r: &CasesReport) -> String {
    let opt = |v: Option<u64>| v.map(human_bytes).unwrap_or_else(|| "—".into());
    let mut md = String::new();
    md.push_str("# SteamPipe-style Model & Pack Pathology Benchmark\n\n");
    md.push_str(&format!("> {}\n\n", r.note));
    md.push_str(&format!(
        "Deterministic datasets (seed {}, {} × 1 MiB assets per pack).\n\n",
        r.seed, r.assets_per_pack
    ));
    md.push_str("| Case | New size | SteamPipe-style | Changed chunks | Fixed reuse | CDC reuse | CAVS .cavsplan | butler | bsdiff | xdelta3 | Diagnosis |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for res in &r.results {
        md.push_str(&format!(
            "| {} | {} | {} | {} of {} | {:.1}% | {:.1}% | {} | {} | {} | {} | {} |\n",
            res.case,
            human_bytes(res.new_bytes),
            human_bytes(res.steampipe_estimate_bytes),
            res.steampipe_changed_chunks,
            res.steampipe_total_chunks,
            res.fixed_reuse_pct,
            res.cdc_reuse_pct,
            human_bytes(res.cavsplan_bytes),
            opt(res.butler_bytes),
            opt(res.bsdiff_bytes),
            opt(res.xdelta3_bytes),
            res.diagnosis,
        ));
    }
    md.push_str("\n## Case descriptions\n\n");
    for res in &r.results {
        md.push_str(&format!("- **{}** — {}\n", res.case, res.description));
    }
    if !r.skipped.is_empty() {
        md.push('\n');
        for s in &r.skipped {
            md.push_str(&format!("> skipped: {s}\n"));
        }
    }
    md
}
