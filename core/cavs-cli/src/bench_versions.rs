//! `cavs bench version-stream` — where package-once-per-release matters.
//!
//! Generates a deterministic v1→vN release stream (~3% of blocks change
//! per release, cumulative), then compares storage and served bytes:
//!
//! - **CAVS**: every version ingested into one content-addressed packfile
//!   store. Any jump (adjacent, v1→vN, reinstall) is served from the same
//!   immutable objects — no per-pair work.
//! - **Pairwise patches** (bsdiff when available): adjacent patches serve
//!   adjacent jumps only; a v1→vN jump needs a dedicated patch (O(N²) to
//!   cover all pairs) or chain-applying every intermediate patch.

use crate::pack::{self, PackOptions};
use crate::report::human_bytes;
use crate::synth::{block_bytes, Rng};
use crate::ChunkModeArg;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;

const BLOCK: usize = 64 * 1024;

#[derive(Default, serde::Serialize)]
struct StreamReport {
    versions: usize,
    version_bytes: u64,
    /// Per adjacent update: compressed new-chunk bytes CAVS serves.
    cavs_adjacent_served: Vec<u64>,
    /// Chunks of vN a v1 client does not have (compressed).
    cavs_jump_v1_to_vn_served: u64,
    cavs_jump_v3_to_vn_served: u64,
    cavs_store_bytes: u64,
    cavs_store_packfiles: u64,
    /// bsdiff adjacent patch sizes (when bsdiff is available).
    bsdiff_adjacent_bytes: Vec<u64>,
    bsdiff_jump_v1_to_vn_bytes: Option<u64>,
    bsdiff_all_pairs_count: u64,
    notes: Vec<String>,
}

pub fn bench(out: &Path, size: &str, versions: usize, seed: u64) -> Result<()> {
    let total = crate::synth::parse_size_pub(size)?;
    let blocks = total.div_ceil(BLOCK as u64).max(4);
    let versions = versions.clamp(2, 32);
    std::fs::create_dir_all(out)?;

    // Which blocks change at each release (cumulative content evolution).
    let changed_at: Vec<HashSet<u64>> = (1..versions)
        .map(|k| {
            let mut rng = Rng::new(seed.wrapping_mul(131).wrapping_add(k as u64));
            let target = (blocks * 3 / 100).max(1);
            let mut set = HashSet::new();
            while (set.len() as u64) < target {
                set.insert(rng.next() % blocks);
            }
            set
        })
        .collect();
    // Block i's content in version v = seeded by the latest release ≤ v
    // that touched it.
    let salt_of = |v: usize, i: u64| -> u64 {
        (1..=v)
            .rev()
            .find(|&k| k >= 1 && changed_at[k - 1].contains(&i))
            .unwrap_or(0) as u64
    };
    let write_version = |v: usize, path: &Path| -> Result<()> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        for i in 0..blocks {
            file.write_all(&block_bytes(seed, salt_of(v, i), i))?;
        }
        file.flush()?;
        Ok(())
    };

    let mut report = StreamReport {
        versions,
        version_bytes: blocks * BLOCK as u64,
        ..Default::default()
    };

    // ---- CAVS: pack every version into one store -------------------------
    let opts = PackOptions {
        segment_time: 4.0,
        mode: Some(ChunkModeArg::Cdc),
        chunk_size: Some(BLOCK),
        profile: None,
        prev: None,
        bootstrap: false,
        compress: true,
        zstd_level: 3,
        force_transcode: false,
        sign_key: None,
        against_signature: None,
    };
    let store_dir = out.join("store");
    if store_dir.exists() {
        std::fs::remove_dir_all(&store_dir)?;
    }
    let mut prev_hashes: Option<HashSet<[u8; 32]>> = None;
    let mut v1_hashes: HashSet<[u8; 32]> = HashSet::new();
    let mut v3_hashes: HashSet<[u8; 32]> = HashSet::new();
    let mut chunk_store_len: std::collections::HashMap<[u8; 32], u64> = Default::default();

    for v in 0..versions {
        let bin = out.join(format!("v{}.bin", v + 1));
        write_version(v, &bin)?;
        let cavs = out.join(format!("v{}.cavs", v + 1));
        pack::pack_raw(std::slice::from_ref(&bin), &cavs, &opts)
            .with_context(|| format!("packing v{}", v + 1))?;
        crate::store::add(
            &store_dir,
            &format!("v{}", v + 1),
            &cavs,
            Some(crate::StorageArg::Packfiles),
        )?;

        let reader = cavs_format::Reader::open(&cavs)?;
        let chunks: Vec<_> = reader.chunks().to_vec();
        let hashes: HashSet<[u8; 32]> = chunks.iter().map(|c| c.hash).collect();
        for c in &chunks {
            chunk_store_len.entry(c.hash).or_insert(c.len_stored as u64);
        }
        if let Some(prev) = &prev_hashes {
            let served: u64 = chunks
                .iter()
                .filter(|c| !prev.contains(&c.hash))
                .map(|c| c.len_stored as u64)
                .sum();
            report.cavs_adjacent_served.push(served);
        }
        if v == 0 {
            v1_hashes = hashes.clone();
        }
        if v == 2 {
            v3_hashes = hashes.clone();
        }
        if v == versions - 1 {
            report.cavs_jump_v1_to_vn_served = chunks
                .iter()
                .filter(|c| !v1_hashes.contains(&c.hash))
                .map(|c| c.len_stored as u64)
                .sum();
            report.cavs_jump_v3_to_vn_served = chunks
                .iter()
                .filter(|c| !v3_hashes.contains(&c.hash))
                .map(|c| c.len_stored as u64)
                .sum();
        }
        prev_hashes = Some(hashes);
        std::fs::remove_file(&cavs)?;
        eprintln!("[stream] v{} packed and ingested", v + 1);
    }
    for entry in walkdir(&store_dir)? {
        report.cavs_store_bytes += std::fs::metadata(&entry)?.len();
        if entry.extension().is_some_and(|e| e == "cavspack") {
            report.cavs_store_packfiles += 1;
        }
    }

    // ---- Pairwise: bsdiff adjacent + one long jump ------------------------
    report.bsdiff_all_pairs_count = (versions as u64) * (versions as u64 - 1) / 2;
    if crate::tool_metrics::available("bsdiff") {
        for v in 1..versions {
            let old = out.join(format!("v{v}.bin"));
            let new = out.join(format!("v{}.bin", v + 1));
            let patch = out.join(format!("v{v}_to_v{}.bsdiff", v + 1));
            let status = std::process::Command::new("bsdiff")
                .args([old.as_os_str(), new.as_os_str(), patch.as_os_str()])
                .status()?;
            if status.success() {
                report
                    .bsdiff_adjacent_bytes
                    .push(std::fs::metadata(&patch)?.len());
            }
            let _ = std::fs::remove_file(&patch);
            eprintln!("[stream] bsdiff v{v}→v{}", v + 1);
        }
        let old = out.join("v1.bin");
        let new = out.join(format!("v{versions}.bin"));
        let patch = out.join("v1_to_vn.bsdiff");
        if std::process::Command::new("bsdiff")
            .args([old.as_os_str(), new.as_os_str(), patch.as_os_str()])
            .status()?
            .success()
        {
            report.bsdiff_jump_v1_to_vn_bytes = Some(std::fs::metadata(&patch)?.len());
        }
        let _ = std::fs::remove_file(&patch);
    } else {
        report
            .notes
            .push("bsdiff not on PATH; pairwise storage column skipped".into());
    }
    // Version artifacts are only needed while diffing.
    for v in 0..versions {
        let _ = std::fs::remove_file(out.join(format!("v{}.bin", v + 1)));
    }

    print_report(&report);
    std::fs::write(
        out.join("version-stream.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(out.join("version-stream.md"), markdown(&report))?;
    println!("results : {}/version-stream.md + .json", out.display());
    Ok(())
}

fn walkdir(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn print_report(r: &StreamReport) {
    println!(
        "bench version-stream: {} versions × {}",
        r.versions,
        human_bytes(r.version_bytes)
    );
    let adj_total: u64 = r.cavs_adjacent_served.iter().sum();
    println!(
        "  CAVS store           : {} in {} packfiles (all {} versions served from it)",
        human_bytes(r.cavs_store_bytes),
        r.cavs_store_packfiles,
        r.versions
    );
    println!(
        "  CAVS adjacent updates: {} total ({} avg per release)",
        human_bytes(adj_total),
        human_bytes(adj_total / r.cavs_adjacent_served.len().max(1) as u64)
    );
    println!(
        "  CAVS jump v1→v{}     : {} (no extra server work)",
        r.versions,
        human_bytes(r.cavs_jump_v1_to_vn_served)
    );
    println!(
        "  CAVS jump v3→v{}     : {}",
        r.versions,
        human_bytes(r.cavs_jump_v3_to_vn_served)
    );
    if !r.bsdiff_adjacent_bytes.is_empty() {
        let total: u64 = r.bsdiff_adjacent_bytes.iter().sum();
        println!(
            "  bsdiff adjacent      : {} total across {} patches",
            human_bytes(total),
            r.bsdiff_adjacent_bytes.len()
        );
        if let Some(jump) = r.bsdiff_jump_v1_to_vn_bytes {
            println!(
                "  bsdiff v1→v{}        : {} (dedicated pair patch)",
                r.versions,
                human_bytes(jump)
            );
        }
        println!(
            "  bsdiff all pairs     : {} patches would be needed to serve any jump directly",
            r.bsdiff_all_pairs_count
        );
    }
    for n in &r.notes {
        println!("  note: {n}");
    }
}

fn markdown(r: &StreamReport) -> String {
    let adj_total: u64 = r.cavs_adjacent_served.iter().sum();
    let mut md = String::new();
    md.push_str("# Many-version release stream\n\n");
    md.push_str(&format!(
        "{} versions of a {} build; ~3% of blocks change per release.\n\n",
        r.versions,
        human_bytes(r.version_bytes)
    ));
    md.push_str("| Method | Storage | Adjacent updates | v1→vN jump | Any-pair coverage |\n|---|---:|---:|---:|---|\n");
    md.push_str(&format!(
        "| CAVS packfile store | {} ({} packfiles) | {} total | {} | every pair, same objects |\n",
        human_bytes(r.cavs_store_bytes),
        r.cavs_store_packfiles,
        human_bytes(adj_total),
        human_bytes(r.cavs_jump_v1_to_vn_served),
    ));
    if !r.bsdiff_adjacent_bytes.is_empty() {
        let total: u64 = r.bsdiff_adjacent_bytes.iter().sum();
        md.push_str(&format!(
            "| bsdiff patches | {} ({} adjacent patches) + full artifacts | {} total | {} (dedicated patch) | needs {} patches (O(N²)) or chain-apply |\n",
            human_bytes(total),
            r.bsdiff_adjacent_bytes.len(),
            human_bytes(total),
            r.bsdiff_jump_v1_to_vn_bytes.map(human_bytes).unwrap_or_else(|| "—".into()),
            r.bsdiff_all_pairs_count,
        ));
    }
    md.push_str(&format!(
        "\nCAVS jump v3→v{}: {}. Adjacent per release (CAVS): {}.\n",
        r.versions,
        human_bytes(r.cavs_jump_v3_to_vn_served),
        r.cavs_adjacent_served
            .iter()
            .map(|&b| human_bytes(b))
            .collect::<Vec<_>>()
            .join(", "),
    ));
    for n in &r.notes {
        md.push_str(&format!("\n> {n}\n"));
    }
    md
}
