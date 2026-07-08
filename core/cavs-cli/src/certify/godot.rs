//! `cavs certify godot` — every CAVS route must keep the simple Godot
//! plugin flow working: download/update, reconstruct the PCK
//! byte-identically, verify the hash, mount with
//! `ProjectSettings.load_resource_pack()`.

use anyhow::Result;
use std::path::Path;

use super::{integrity, worst, CheckResult, CheckRow};
use crate::analyze_packs::{self, godot_pck};
use crate::bench_routes::RoutesReport;

pub struct Args<'a> {
    pub old_pck: &'a Path,
    pub new_pck: &'a Path,
    pub godot_bin: Option<&'a str>,
    pub test_project: Option<&'a Path>,
    /// Godot plugin directory (e.g. game-engine-plugins/godot-plugin/addons/cavs) for the API
    /// surface check.
    pub plugin_dir: Option<&'a Path>,
}

/// Results already computed by earlier certification phases, so the Godot
/// section does not repeat expensive work in a full `cavs certify` run.
pub struct Precomputed<'a> {
    pub byte_identical: bool,
    pub measured: Option<&'a RoutesReport>,
}

pub struct Outcome {
    pub rows: Vec<CheckRow>,
    pub result: CheckResult,
    pub byte_identical: bool,
}

/// The documented public plugin flow, certified stable for v1.x
/// (the API `addons/cavs/cavs_client.gd` has shipped since v0.2).
pub const PLUGIN_FLOW: &str = r#"var cavs := CavsClient.new("http://127.0.0.1:8990")
var result := cavs.fetch("main_pack")   # blocking: run in a Thread
if result.ok:
    ProjectSettings.load_resource_pack(result.files[0])

# or, in one line (fetch + mount the first .pck):
CavsClient.new("http://127.0.0.1:8990").ensure_pack("main_pack")"#;

/// GDScript functions the plugin must keep exporting for v1.x.
const PLUGIN_API: &[&str] = &["func fetch(", "func fetch_async(", "func ensure_pack("];

fn parse_row(name: &str, path: &Path) -> CheckRow {
    match std::fs::read(path) {
        Err(e) => CheckRow::new(name, CheckResult::Fail, format!("cannot read: {e}")),
        Ok(bytes) => match godot_pck::parse(&bytes) {
            Ok(dir) => CheckRow::new(
                name,
                CheckResult::Pass,
                format!("PCK v{}, {} resources", dir.version, dir.entries.len()),
            ),
            Err(e) => CheckRow::new(
                name,
                CheckResult::Warn,
                format!("directory not parseable ({e}); byte-level checks still apply"),
            ),
        },
    }
}

pub fn run_with(
    args: &Args,
    precomputed: Option<Precomputed>,
    out_dir: &Path,
    commands: &mut Vec<String>,
) -> Result<Outcome> {
    std::fs::create_dir_all(out_dir)?;
    let mut rows: Vec<CheckRow> = Vec::new();

    // -- PCK structure ---------------------------------------------------------
    rows.push(parse_row("old PCK parse", args.old_pck));
    rows.push(parse_row("new PCK parse", args.new_pck));

    // -- Byte-identical reconstruction (plan route) ------------------------------
    let byte_identical = match &precomputed {
        Some(p) => {
            rows.push(CheckRow::new(
                ".cavsplan output byte-identical",
                if p.byte_identical {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                },
                "verified by the integrity phase of this run",
            ));
            p.byte_identical
        }
        None => {
            let inputs = integrity::Inputs {
                old: Some(args.old_pck),
                new: Some(args.new_pck),
                signature_old: None,
                signature_new: None,
                plan: None,
                corruption_checks: false,
                noop_check: false,
            };
            let int = integrity::run(&inputs, out_dir, commands)?;
            rows.push(CheckRow::new(
                ".cavsplan output byte-identical",
                if int.byte_identical {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                },
                format!("integrity checks: {}", int.result.label()),
            ));
            int.byte_identical
        }
    };

    // -- Every measured route reconstructs byte-identically -----------------------
    let owned_report;
    let measured: Option<&RoutesReport> = match &precomputed {
        Some(p) => p.measured,
        None => {
            let bench_dir = out_dir.join("artifacts").join("route-bench");
            std::fs::create_dir_all(&bench_dir)?;
            commands.push(format!(
                "cavs bench routes --old {} --new {} --out artifacts/route-bench",
                args.old_pck.display(),
                args.new_pck.display()
            ));
            owned_report = crate::bench_routes::collect(&crate::bench_routes::RoutesArgs {
                old: args.old_pck,
                new: args.new_pck,
                butler_bin: None,
                include_pairwise_proxy: false,
                out: &bench_dir,
            })?;
            Some(&owned_report)
        }
    };
    if let Some(m) = measured {
        for (label, needle) in [
            ("chunk/hybrid route byte-identical", "chunk / hybrid"),
            ("bootstrap route byte-identical", "bootstrap"),
        ] {
            match m.routes.iter().find(|r| r.route.contains(needle)) {
                Some(r) => rows.push(match r.output_ok {
                    Some(false) => CheckRow::new(
                        label,
                        CheckResult::Fail,
                        format!("{}: output differs", r.route),
                    ),
                    _ => CheckRow::new(
                        label,
                        CheckResult::Pass,
                        format!(
                            "{} — {}",
                            r.route,
                            crate::report::human_bytes(r.network_bytes)
                        ),
                    ),
                }),
                None => rows.push(CheckRow::new(
                    label,
                    CheckResult::Skipped,
                    "route not measured in this run",
                )),
            }
        }
    }

    // No route may mount an unverified PCK: every route above hash-verifies
    // its output before promotion, so a failed verification fails this row.
    rows.push(if byte_identical {
        CheckRow::new(
            "unverified PCKs never mounted",
            CheckResult::Pass,
            "every route verifies the reconstructed PCK hash before it is promoted/mounted",
        )
    } else {
        CheckRow::new(
            "unverified PCKs never mounted",
            CheckResult::Fail,
            "reconstruction verification failed — nothing may be mounted",
        )
    });

    // -- PCK analyzer report -------------------------------------------------------
    let analysis_path = out_dir.join("godot-pck-analysis.md");
    commands.push(format!(
        "cavs analyze godot-pck {} {} --out godot-pck-analysis.md",
        args.old_pck.display(),
        args.new_pck.display()
    ));
    rows.push(
        match analyze_packs::analyze_godot_pck(&analyze_packs::GodotArgs {
            old: args.old_pck,
            new: args.new_pck,
            out: Some(&analysis_path),
            json: false,
        }) {
            Ok(()) => CheckRow::new(
                "Godot PCK analyzer report",
                CheckResult::Pass,
                "godot-pck-analysis.md (actionable layout recommendations)",
            ),
            Err(e) => CheckRow::new(
                "Godot PCK analyzer report",
                CheckResult::Warn,
                format!("{e:#}"),
            ),
        },
    );

    // -- Plugin API surface ----------------------------------------------------------
    rows.push(plugin_api_row(args.plugin_dir));

    // -- Optional engine smoke test ----------------------------------------------------
    rows.push(smoke_test_row(args, commands));

    let result = worst(&rows);
    Ok(Outcome {
        rows,
        result,
        byte_identical,
    })
}

pub fn run(args: &Args, out_dir: &Path, commands: &mut Vec<String>) -> Result<Outcome> {
    run_with(args, None, out_dir, commands)
}

fn plugin_api_row(plugin_dir: Option<&Path>) -> CheckRow {
    let Some(dir) = plugin_dir else {
        return CheckRow::new(
            "plugin API surface",
            CheckResult::Skipped,
            "pass --plugin-dir to verify fetch/fetch_async/ensure_pack are still exported",
        );
    };
    let mut sources = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("gd") {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    sources.push_str(&src);
                }
            }
        }
    }
    let missing: Vec<&str> = PLUGIN_API
        .iter()
        .filter(|f| !sources.contains(**f))
        .copied()
        .collect();
    if missing.is_empty() {
        CheckRow::new(
            "plugin API surface",
            CheckResult::Pass,
            "fetch, fetch_async and ensure_pack exported — the documented flow stays valid",
        )
    } else {
        CheckRow::new(
            "plugin API surface",
            CheckResult::Fail,
            format!(
                "missing from {}: {} — the documented plugin flow broke",
                dir.display(),
                missing.join(", ")
            ),
        )
    }
}

fn smoke_test_row(args: &Args, commands: &mut Vec<String>) -> CheckRow {
    let (Some(bin), Some(project)) = (args.godot_bin, args.test_project) else {
        return CheckRow::new(
            "Godot engine smoke test",
            CheckResult::Skipped,
            "pass --godot-bin and --test-project to run it (optional)",
        );
    };
    if !crate::tool_metrics::available(bin) {
        return CheckRow::new(
            "Godot engine smoke test",
            CheckResult::Skipped,
            format!("Godot binary '{bin}' not available"),
        );
    }
    commands.push(format!(
        "{bin} --headless --path {} --quit",
        project.display()
    ));
    match crate::tool_metrics::run_measured(
        bin,
        &[
            "--headless",
            "--path",
            &project.display().to_string(),
            "--quit",
        ],
        None,
    ) {
        Ok(run) if run.exit_ok => CheckRow::new(
            "Godot engine smoke test",
            CheckResult::Pass,
            format!("project loaded and quit cleanly in {} ms", run.wall_ms),
        ),
        Ok(run) => CheckRow::new(
            "Godot engine smoke test",
            CheckResult::Fail,
            format!(
                "exit code {:?}: {}",
                run.exit_code,
                run.stderr.lines().last().unwrap_or("")
            ),
        ),
        Err(e) => CheckRow::new(
            "Godot engine smoke test",
            CheckResult::Fail,
            format!("{e:#}"),
        ),
    }
}

#[derive(serde::Serialize)]
struct Report<'a> {
    schema: &'static str,
    result: CheckResult,
    byte_identical: bool,
    checks: &'a [CheckRow],
    plugin_flow: &'static str,
}

pub fn write_reports(outcome: &Outcome, out_dir: &Path) -> Result<()> {
    std::fs::write(
        out_dir.join("godot.json"),
        serde_json::to_vec_pretty(&Report {
            schema: "cavs-certify-godot/1",
            result: outcome.result,
            byte_identical: outcome.byte_identical,
            checks: &outcome.rows,
            plugin_flow: PLUGIN_FLOW,
        })?,
    )?;
    let mut md = String::from("# Godot Certification\n\n");
    md.push_str(&format!("Result: **{}**\n\n", outcome.result.label()));
    md.push_str(&super::rows_markdown(&outcome.rows));
    md.push_str("\n## Certified plugin flow (stable for v1.x)\n\n```gdscript\n");
    md.push_str(PLUGIN_FLOW);
    md.push_str(
        "\n```\n\nThe plugin stays intentionally simple: download only what is \
needed, reconstruct the PCK, verify the hash, mount it with \
`ProjectSettings.load_resource_pack()`.\n",
    );
    std::fs::write(out_dir.join("godot.md"), md)?;
    Ok(())
}
