//! `cavs-steam` — SteamPipe Update Analyzer.
//!
//! Compares two game builds, estimates the SteamPipe patch size, flags the
//! pack files and changes that cause update bloat, and recommends packaging
//! fixes — before you publish. Estimates are predictive models, not official
//! Steam output. Reuses the CAVS chunking (FastCDC) and hashing (BLAKE3).

mod analyze;
mod report;

use analyze::{compare, Engine, RiskLevel};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use report::human;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "cavs-steam",
    version,
    about = "Find Steam update bloat before your players download it",
    long_about = "Analyzes two game builds and estimates the SteamPipe patch size, \
                  detects problematic pack files (reordering, offset cascades, oversized \
                  packs), and recommends packaging fixes. Estimates are predictive models, \
                  not official Steam results.",
    after_help = "EXAMPLES:\n  \
        cavs-steam compare ./build_v1 ./build_v2 --out report\n  \
        cavs-steam ci ./prev ./current --max-estimated-update 500MiB --max-risk high\n"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare two builds and write a full report (HTML/MD/JSON/CSV).
    Compare {
        old_build: PathBuf,
        new_build: PathBuf,
        /// Output directory for the report.
        #[arg(short, long, default_value = "cavs-steam-report")]
        out: PathBuf,
        /// Engine hint for tailored recommendations.
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
        /// Comma-separated report formats.
        #[arg(long, default_value = "html,md,json,csv")]
        formats: String,
    },
    /// Compare and fail (non-zero exit) if the update or risk exceeds a
    /// budget — for CI gates.
    Ci {
        old_build: PathBuf,
        new_build: PathBuf,
        /// Fail if the estimated SteamPipe update exceeds this (e.g. 500MiB).
        #[arg(long)]
        max_estimated_update: Option<String>,
        /// Fail if overall risk is at or above this level.
        #[arg(long, value_enum, default_value = "high")]
        max_risk: RiskArg,
        #[arg(short, long, default_value = "cavs-steam-report")]
        out: PathBuf,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum EngineArg {
    Auto,
    Unreal,
    Unity,
    Godot,
    Custom,
}
impl From<EngineArg> for Engine {
    fn from(e: EngineArg) -> Self {
        match e {
            EngineArg::Auto => Engine::Auto,
            EngineArg::Unreal => Engine::Unreal,
            EngineArg::Unity => Engine::Unity,
            EngineArg::Godot => Engine::Godot,
            EngineArg::Custom => Engine::Custom,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum, PartialEq)]
enum RiskArg {
    Low,
    Medium,
    High,
}
impl RiskArg {
    fn level(self) -> RiskLevel {
        match self {
            RiskArg::Low => RiskLevel::Low,
            RiskArg::Medium => RiskLevel::Medium,
            RiskArg::High => RiskLevel::High,
        }
    }
}

/// Parse "500MiB", "2GiB", "1024", "300KiB"... into bytes.
fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("GiB") {
        (n, 1u64 << 30)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1 << 20)
    } else if let Some(n) = s.strip_suffix("KiB") {
        (n, 1 << 10)
    } else if let Some(n) = s.strip_suffix("B") {
        (n, 1)
    } else {
        (s, 1)
    };
    let v: f64 = num.trim().parse().with_context(|| format!("bad size: {s}"))?;
    Ok((v * mult as f64) as u64)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(3) // tool error
        }
    }
}

fn run() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::Compare {
            old_build,
            new_build,
            out,
            engine,
            formats,
        } => {
            let report = compare(&old_build, &new_build, engine.into())?;
            let fmts: Vec<String> = formats.split(',').map(|s| s.trim().to_string()).collect();
            report::write_all(&report, &out, &fmts)?;
            print_summary(&report);
            eprintln!("[cavs-steam] report written to {}", out.display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Ci {
            old_build,
            new_build,
            max_estimated_update,
            max_risk,
            out,
            engine,
        } => {
            let report = compare(&old_build, &new_build, engine.into())?;
            report::write_all(&report, &out, &["json".into(), "md".into(), "html".into()])?;
            print_summary(&report);

            let mut fail = false;
            if let Some(budget) = &max_estimated_update {
                let budget = parse_size(budget)?;
                if report.estimated_steam_update_bytes > budget {
                    eprintln!(
                        "[FAIL] estimated update {} exceeds budget {}",
                        human(report.estimated_steam_update_bytes),
                        human(budget)
                    );
                    fail = true;
                }
            }
            if report.risk.rank() >= max_risk.level().rank() {
                eprintln!(
                    "[FAIL] risk {} at or above threshold {}",
                    report.risk.label(),
                    max_risk.level().label()
                );
                fail = true;
            }
            if fail {
                Ok(ExitCode::from(2)) // hard failure
            } else {
                eprintln!("[cavs-steam] within budget");
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}

fn print_summary(r: &analyze::Report) {
    println!("build   : {} -> {}", human(r.old_size_bytes), human(r.new_size_bytes));
    println!(
        "changed : {} files changed, {} new",
        r.changed_files, r.new_files
    );
    println!(
        "steam   : {} estimated update (reuse {:.1}%)",
        human(r.estimated_steam_update_bytes),
        r.steam_reuse_ratio * 100.0
    );
    println!(
        "cavs    : {} FastCDC 64 KiB estimate (reuse {:.1}%)",
        human(r.estimated_cdc_update_bytes),
        r.cdc_reuse_ratio * 100.0
    );
    println!("risk    : {}", r.risk.label().to_uppercase());
    if let Some(top) = r.top_offenders.first() {
        println!(
            "offender: {} ({} steam update, {})",
            top.path,
            human(top.steam_payload_compressed),
            top.reasons.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("500MiB").unwrap(), 500 << 20);
        assert_eq!(parse_size("2GiB").unwrap(), 2 << 30);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("300KiB").unwrap(), 300 << 10);
    }
}
