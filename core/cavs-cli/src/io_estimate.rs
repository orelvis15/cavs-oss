//! `cavs io-estimate` (v0.9.0): local disk I/O cost of an update, per
//! delivery route. Download size is not the only cost — a fixed-chunk
//! updater rebuilds every touched file alongside the old one, so a 25 GiB
//! pack with 10 changed bytes still costs ~50 GiB of local I/O. This
//! command estimates that pain per route and per storage device.

use crate::report::human_bytes;
use anyhow::Result;
use cavs_analyzer::compare::{analyze, Analysis};
use cavs_analyzer::detect::Thresholds;
use cavs_analyzer::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct DeviceProfile {
    pub sequential_read_mb_s: f64,
    pub sequential_write_mb_s: f64,
    pub seek_ms: f64,
}

/// The plan's default profiles.
pub fn default_profiles() -> BTreeMap<String, DeviceProfile> {
    BTreeMap::from([
        (
            "hdd".into(),
            DeviceProfile {
                sequential_read_mb_s: 120.0,
                sequential_write_mb_s: 100.0,
                seek_ms: 8.0,
            },
        ),
        (
            "sata_ssd".into(),
            DeviceProfile {
                sequential_read_mb_s: 500.0,
                sequential_write_mb_s: 450.0,
                seek_ms: 0.1,
            },
        ),
        (
            "nvme".into(),
            DeviceProfile {
                sequential_read_mb_s: 3500.0,
                sequential_write_mb_s: 2500.0,
                seek_ms: 0.02,
            },
        ),
    ])
}

pub fn load_profiles(path: Option<&Path>) -> Result<BTreeMap<String, DeviceProfile>> {
    match path {
        Some(p) => {
            let raw = std::fs::read_to_string(p)?;
            Ok(toml::from_str(&raw)?)
        }
        None => Ok(default_profiles()),
    }
}

#[derive(Serialize, Clone)]
pub struct RouteIo {
    pub route: String,
    pub download_bytes: u64,
    pub read_old_bytes: u64,
    pub write_bytes: u64,
    pub temp_disk_bytes: u64,
    pub file_creates: u64,
    pub file_renames: u64,
    pub file_deletes: u64,
    pub seeks: u64,
    /// Whether local I/O dwarfs what the route saves on the network.
    pub io_dominates_network: bool,
    /// Estimated seconds per device profile.
    pub device_seconds: BTreeMap<String, f64>,
    pub notes: String,
}

#[derive(Serialize)]
pub struct IoReport {
    pub old: String,
    pub new: String,
    pub routes: Vec<RouteIo>,
    pub note: String,
}

pub struct IoArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub device_profiles: Option<&'a Path>,
    pub out: Option<&'a Path>,
    pub json: bool,
}

fn seconds(p: &DeviceProfile, download: u64, read: u64, write: u64, seeks: u64) -> f64 {
    // Downloads also land on disk; count them as writes.
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    mb(read) / p.sequential_read_mb_s
        + (mb(write) + mb(download)) / p.sequential_write_mb_s
        + seeks as f64 * p.seek_ms / 1000.0
}

#[allow(clippy::too_many_arguments)]
fn route(
    profiles: &BTreeMap<String, DeviceProfile>,
    full_new_size: u64,
    name: &str,
    download: u64,
    read_old: u64,
    write: u64,
    temp: u64,
    creates: u64,
    renames: u64,
    deletes: u64,
    seeks: u64,
    notes: &str,
) -> RouteIo {
    let device_seconds = profiles
        .iter()
        .map(|(n, p)| (n.clone(), seconds(p, download, read_old, write, seeks)))
        .collect();
    // The route "saves" (full − download) network bytes; when its local
    // I/O exceeds the whole build size it dominates that saving.
    let io = read_old + write;
    RouteIo {
        route: name.into(),
        download_bytes: download,
        read_old_bytes: read_old,
        write_bytes: write,
        temp_disk_bytes: temp,
        file_creates: creates,
        file_renames: renames,
        file_deletes: deletes,
        seeks,
        io_dominates_network: io > full_new_size.max(1),
        device_seconds,
        notes: notes.into(),
    }
}

pub fn routes_from_analysis(
    a: &Analysis,
    profiles: &BTreeMap<String, DeviceProfile>,
) -> Vec<RouteIo> {
    let touched_new: u64 = a.files.iter().map(|f| f.new_size).sum();
    let touched_old: u64 = a.files.iter().map(|f| f.old_size).sum();
    let touched = a.files.len() as u64;
    let creates = a.files_added as u64;
    let deletes = a.files_deleted as u64;
    let all_files = (a.files_unchanged + a.files_modified + a.files_added) as u64;
    let changed_regions: u64 = a.files.iter().map(|f| f.heat_1m.runs).sum();
    let largest_touched: u64 = a.files.iter().map(|f| f.new_size).max().unwrap_or(0);

    vec![
        route(
            profiles,
            a.new_size_bytes,
            "full download (raw)",
            a.new_size_bytes,
            0,
            0,
            a.new_size_bytes,
            all_files,
            all_files,
            deletes,
            all_files,
            "downloads land in staging, then rename",
        ),
        route(
            profiles,
            a.new_size_bytes,
            "SteamPipe-style (fixed 1 MiB)",
            a.estimated_steampipe_download,
            touched_old,
            touched_new,
            touched_new,
            creates,
            touched,
            deletes,
            touched * 2 + changed_regions,
            "rebuilds every touched file alongside the old one, commits at the end",
        ),
        route(
            profiles,
            a.new_size_bytes,
            "CAVS chunks / hybrid",
            a.estimated_cavs_download,
            touched_old,
            touched_new,
            touched_new,
            creates,
            touched,
            deletes,
            touched * 2 + changed_regions,
            "old install + cache as local sources, staged per file",
        ),
        route(
            profiles,
            a.new_size_bytes,
            "CAVS .cavsplan",
            a.estimated_cavs_download,
            touched_old,
            touched_new,
            largest_touched,
            creates,
            touched,
            deletes,
            touched * 2 + changed_regions,
            "journaled per-file staging: temp peak is the largest touched file",
        ),
    ]
}

pub fn io_estimate(args: &IoArgs) -> Result<()> {
    let profiles = load_profiles(args.device_profiles)?;
    let analysis = analyze(
        args.old,
        args.new,
        Engine::Auto,
        &Thresholds::default(),
        &|_: &str| true,
    )?;
    let report = IoReport {
        old: analysis.old_build.clone(),
        new: analysis.new_build.clone(),
        routes: routes_from_analysis(&analysis, &profiles),
        note: "I/O figures are estimates from the update model; device times assume \
               sequential throughput plus per-seek latency."
            .into(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("io-estimate: {} → {}", report.old, report.new);
        for r in &report.routes {
            println!(
                "  {:<30} dl {:>10}  read {:>10}  write {:>10}  temp {:>10}{}",
                r.route,
                human_bytes(r.download_bytes),
                human_bytes(r.read_old_bytes),
                human_bytes(r.write_bytes),
                human_bytes(r.temp_disk_bytes),
                if r.io_dominates_network {
                    "  [local I/O dominates]"
                } else {
                    ""
                }
            );
            let times: Vec<String> = r
                .device_seconds
                .iter()
                .map(|(d, s)| format!("{d} {}", human_secs(*s)))
                .collect();
            println!("  {:<30} {}", "", times.join("  ·  "));
        }
    }
    if let Some(path) = args.out {
        std::fs::write(path, markdown(&report))?;
        eprintln!("report  : {}", path.display());
    }
    Ok(())
}

pub fn human_secs(s: f64) -> String {
    if s >= 60.0 {
        format!("{}m {:02.0}s", (s / 60.0) as u64, s % 60.0)
    } else if s >= 1.0 {
        format!("{s:.1}s")
    } else {
        format!("{:.0}ms", s * 1000.0)
    }
}

pub fn markdown(r: &IoReport) -> String {
    let mut md = String::new();
    md.push_str("# Local Disk I/O Estimate\n\n");
    md.push_str(&format!("> {}\n\n", r.note));
    md.push_str(&format!("`{}` → `{}`\n\n", r.old, r.new));
    let devices: Vec<&String> = r
        .routes
        .first()
        .map(|route| route.device_seconds.keys().collect())
        .unwrap_or_default();
    md.push_str(
        "| Route | Download | Read old | Write | Temp required | Creates | Renames | Deletes |",
    );
    for d in &devices {
        md.push_str(&format!(" {d} est. |"));
    }
    md.push_str("\n|---|---:|---:|---:|---:|---:|---:|---:|");
    for _ in &devices {
        md.push_str("---:|");
    }
    md.push('\n');
    for route in &r.routes {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            route.route,
            human_bytes(route.download_bytes),
            human_bytes(route.read_old_bytes),
            human_bytes(route.write_bytes),
            human_bytes(route.temp_disk_bytes),
            route.file_creates,
            route.file_renames,
            route.file_deletes,
        ));
        for d in &devices {
            md.push_str(&format!(
                " {} |",
                human_secs(route.device_seconds.get(*d).copied().unwrap_or(0.0))
            ));
        }
        md.push('\n');
    }
    for route in &r.routes {
        if route.io_dominates_network {
            md.push_str(&format!(
                "\n> **{}**: local I/O ({} read + {} write) exceeds the whole build — \
                 the network saving does not translate into a faster update on slow disks.\n",
                route.route,
                human_bytes(route.read_old_bytes),
                human_bytes(route.write_bytes)
            ));
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiles_match_plan() {
        let p = default_profiles();
        assert_eq!(p["hdd"].sequential_read_mb_s, 120.0);
        assert_eq!(p["nvme"].seek_ms, 0.02);
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn seconds_scale_with_device() {
        let p = default_profiles();
        let slow = seconds(&p["hdd"], 10 << 20, 1 << 30, 1 << 30, 100);
        let fast = seconds(&p["nvme"], 10 << 20, 1 << 30, 1 << 30, 100);
        assert!(slow > fast * 5.0, "hdd {slow} vs nvme {fast}");
    }

    #[test]
    fn custom_profiles_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.toml");
        std::fs::write(
            &path,
            "[usb]\nsequential_read_mb_s = 30\nsequential_write_mb_s = 20\nseek_ms = 12\n",
        )
        .unwrap();
        let p = load_profiles(Some(&path)).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p["usb"].sequential_write_mb_s, 20.0);
    }

    #[test]
    fn human_seconds_formatting() {
        assert_eq!(human_secs(0.2), "200ms");
        assert_eq!(human_secs(2.34), "2.3s");
        assert_eq!(human_secs(460.0), "7m 40s");
    }
}
