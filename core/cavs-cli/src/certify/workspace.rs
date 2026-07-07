//! `cavs certify workspace` — app/depot/branch/build metadata must be
//! valid, promotion/rollback previews must work, install plans must
//! resolve per platform/language/ownership, and depot sharing math must
//! be deterministic.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use cavs_workspace::{sharing, Build, Workspace};

use super::{worst, CheckResult, CheckRow};
use crate::report::human_bytes;
use crate::workspace_cmd::{self, InstallPlanArgs};

pub struct Outcome {
    pub rows: Vec<CheckRow>,
    pub result: CheckResult,
    /// Depots present in both builds with still-existing source dirs, for
    /// per-depot integrity certification: (depot id, old source, new source).
    pub depot_pairs: Vec<(String, PathBuf, PathBuf)>,
    /// (depot id, update bytes from old→new) for the per-depot cost table.
    pub depot_costs: Vec<(String, u64, u64)>,
}

fn find_build(builds: &[Build], key: &str) -> Option<Build> {
    builds
        .iter()
        .find(|b| b.id == key || b.label.as_deref() == Some(key))
        .cloned()
}

pub fn run(
    ws_path: &Path,
    app: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    out_dir: &Path,
    commands: &mut Vec<String>,
) -> Result<Outcome> {
    std::fs::create_dir_all(out_dir)?;
    let mut rows: Vec<CheckRow> = Vec::new();
    let mut depot_pairs = Vec::new();
    let mut depot_costs = Vec::new();

    // -- Metadata -----------------------------------------------------------
    let ws = match Workspace::open(ws_path) {
        Ok(ws) => {
            rows.push(CheckRow::new(
                "metadata parse",
                CheckResult::Pass,
                "workspace.toml valid",
            ));
            ws
        }
        Err(e) => {
            rows.push(CheckRow::new(
                "metadata parse",
                CheckResult::Fail,
                format!("{e:#}"),
            ));
            let result = worst(&rows);
            return Ok(Outcome {
                rows,
                result,
                depot_pairs,
                depot_costs,
            });
        }
    };
    let app_id = ws.app_id(app).context("cannot resolve app")?;
    let app_meta = ws.load_app(&app_id).context("cannot load app")?;
    rows.push(CheckRow::new(
        "app exists",
        CheckResult::Pass,
        format!("app '{app_id}'"),
    ));
    rows.push(if app_meta.depots.is_empty() {
        CheckRow::new("depots exist", CheckResult::Fail, "app has no depots")
    } else {
        CheckRow::new(
            "depots exist",
            CheckResult::Pass,
            format!(
                "{}: {}",
                app_meta.depots.len(),
                app_meta
                    .depots
                    .iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    });

    let builds = ws.builds(&app_id).unwrap_or_default();

    // -- Branches -----------------------------------------------------------
    let mut branch_problems = Vec::new();
    for b in &app_meta.branches {
        if let Some(cur) = &b.current_build {
            if find_build(&builds, cur).is_none() {
                branch_problems.push(format!("branch '{}' points at missing build '{cur}'", b.id));
            }
        }
        for h in &b.history {
            if find_build(&builds, h).is_none() {
                branch_problems.push(format!("branch '{}' history has missing build '{h}'", b.id));
            }
        }
    }
    rows.push(if branch_problems.is_empty() {
        CheckRow::new(
            "branches valid",
            CheckResult::Pass,
            format!(
                "{} branches, every reference resolves",
                app_meta.branches.len()
            ),
        )
    } else {
        CheckRow::new(
            "branches valid",
            CheckResult::Fail,
            branch_problems.join("; "),
        )
    });

    // -- Builds -------------------------------------------------------------
    let from_build = from.and_then(|f| find_build(&builds, f));
    let to_build = match to {
        Some(t) => find_build(&builds, t),
        None => builds.last().cloned(),
    };
    rows.push(match (&from, &from_build) {
        (None, _) => CheckRow::new("from build", CheckResult::Skipped, "no --from given"),
        (Some(f), Some(b)) => CheckRow::new(
            "from build",
            CheckResult::Pass,
            format!("'{f}' → {} ({} depots)", b.id, b.depots.len()),
        ),
        (Some(f), None) => CheckRow::new(
            "from build",
            CheckResult::Fail,
            format!("build '{f}' not found"),
        ),
    });
    rows.push(match (&to, &to_build) {
        (Some(t), Some(b)) => CheckRow::new(
            "to build",
            CheckResult::Pass,
            format!("'{t}' → {} ({} depots)", b.id, b.depots.len()),
        ),
        (None, Some(b)) => CheckRow::new(
            "to build",
            CheckResult::Pass,
            format!("latest build {} ({} depots)", b.id, b.depots.len()),
        ),
        (_, None) => CheckRow::new("to build", CheckResult::Fail, "target build not found"),
    });

    let Some(to_build) = to_build else {
        let result = worst(&rows);
        return Ok(Outcome {
            rows,
            result,
            depot_pairs,
            depot_costs,
        });
    };

    // Depot indices of the target build must load.
    let mut to_indices = Vec::new();
    let mut index_errors = Vec::new();
    for d in &to_build.depots {
        match ws.depot_index(&app_id, &to_build.id, &d.depot_id) {
            Ok(idx) => to_indices.push(idx),
            Err(e) => index_errors.push(format!("{}: {e}", d.depot_id)),
        }
    }
    rows.push(if index_errors.is_empty() {
        CheckRow::new(
            "build depot indices",
            CheckResult::Pass,
            format!("{} indices load", to_indices.len()),
        )
    } else {
        CheckRow::new(
            "build depot indices",
            CheckResult::Fail,
            index_errors.join("; "),
        )
    });

    // -- Promote / rollback previews -----------------------------------------
    if let Some(branch) = app_meta.branches.first() {
        commands.push(format!(
            "cavs branch promote-preview --workspace {} --app {app_id} --branch {} --build {}",
            ws_path.display(),
            branch.id,
            to_build.id
        ));
        rows.push(
            match workspace_cmd::branch_promote_preview(
                ws_path,
                Some(&app_id),
                &branch.id,
                &to_build.id,
                false,
            ) {
                Ok(()) => CheckRow::new(
                    "branch promote preview",
                    CheckResult::Pass,
                    format!("branch '{}' → build {}", branch.id, to_build.id),
                ),
                Err(e) => CheckRow::new(
                    "branch promote preview",
                    CheckResult::Fail,
                    format!("{e:#}"),
                ),
            },
        );
        // Rollback must only target builds the branch served before.
        let rollback_target = branch
            .history
            .iter()
            .rev()
            .find(|h| Some(h.as_str()) != branch.current_build.as_deref());
        rows.push(match rollback_target {
            Some(target) if find_build(&builds, target).is_some() => CheckRow::new(
                "rollback preview",
                CheckResult::Pass,
                format!(
                    "branch '{}' can roll back to previously-served build '{target}'",
                    branch.id
                ),
            ),
            Some(target) => CheckRow::new(
                "rollback preview",
                CheckResult::Fail,
                format!("history entry '{target}' is not a recorded build"),
            ),
            None => CheckRow::new(
                "rollback preview",
                CheckResult::Skipped,
                format!("branch '{}' has no earlier served build", branch.id),
            ),
        });
    } else {
        rows.push(CheckRow::new(
            "branch promote preview",
            CheckResult::Skipped,
            "app has no branches",
        ));
    }

    // -- Depot sharing: report + determinism -----------------------------------
    if to_indices.len() >= 2 {
        let m1 = sharing::matrix(&to_indices);
        let m2 = sharing::matrix(&to_indices);
        let deterministic = m1.len() == m2.len()
            && m1
                .iter()
                .zip(m2.iter())
                .all(|(a, b)| a.shared_bytes == b.shared_bytes);
        commands.push(format!(
            "cavs depot analyze-sharing --workspace {} --app {app_id} --build {} --out depot-sharing.md",
            ws_path.display(),
            to_build.id
        ));
        let report = workspace_cmd::depot_analyze_sharing(
            ws_path,
            Some(&app_id),
            Some(&to_build.id),
            Some(&out_dir.join("depot-sharing.md")),
            false,
        );
        rows.push(match (deterministic, report) {
            (true, Ok(())) => CheckRow::new(
                "depot sharing",
                CheckResult::Pass,
                format!("{} pairs, deterministic; depot-sharing.md", m1.len()),
            ),
            (false, _) => CheckRow::new(
                "depot sharing",
                CheckResult::Fail,
                "sharing matrix is not deterministic across runs",
            ),
            (_, Err(e)) => CheckRow::new("depot sharing", CheckResult::Fail, format!("{e:#}")),
        });
    } else {
        rows.push(CheckRow::new(
            "depot sharing",
            CheckResult::Skipped,
            "needs at least two depots",
        ));
    }

    // -- Per-depot update cost + integrity pairs ---------------------------------
    if let Some(fb) = &from_build {
        for db in &to_build.depots {
            let Some(old_db) = fb.depots.iter().find(|d| d.depot_id == db.depot_id) else {
                continue;
            };
            if let (Ok(new_idx), Ok(old_idx)) = (
                ws.depot_index(&app_id, &to_build.id, &db.depot_id),
                ws.depot_index(&app_id, &fb.id, &db.depot_id),
            ) {
                let cost = sharing::fetch_bytes(&new_idx, &[&old_idx]);
                depot_costs.push((db.depot_id.clone(), cost, new_idx.total_bytes));
            }
            let old_src = PathBuf::from(&old_db.source_path);
            let new_src = PathBuf::from(&db.source_path);
            if old_src.is_dir() && new_src.is_dir() {
                depot_pairs.push((db.depot_id.clone(), old_src, new_src));
            }
        }
        rows.push(CheckRow::new(
            "per-depot update cost",
            CheckResult::Pass,
            depot_costs
                .iter()
                .map(|(d, c, _)| format!("{d}: {}", human_bytes(*c)))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    // -- Install-plan states -------------------------------------------------------
    let plans_dir = out_dir.join("install-plans");
    std::fs::create_dir_all(&plans_dir)?;
    // (label, platform, language, owned depot ids)
    type PlanState = (String, Option<String>, Option<String>, Vec<String>);
    let mut states: Vec<PlanState> = Vec::new();
    let platforms: Vec<String> = {
        let mut p: Vec<String> = app_meta
            .depots
            .iter()
            .filter_map(|d| d.platform.map(|p| format!("{p:?}").to_lowercase()))
            .collect();
        p.sort();
        p.dedup();
        if p.is_empty() {
            vec!["windows".into()]
        } else {
            p
        }
    };
    let base_owned: Vec<String> = app_meta
        .depots
        .iter()
        .filter(|d| !d.optional)
        .map(|d| d.id.clone())
        .collect();
    for platform in &platforms {
        states.push((
            format!("{platform} + base"),
            Some(platform.clone()),
            None,
            base_owned.clone(),
        ));
    }
    for lang_depot in app_meta.depots.iter().filter(|d| d.language.is_some()) {
        let lang = lang_depot.language.clone().unwrap();
        let mut owned = base_owned.clone();
        owned.push(lang_depot.id.clone());
        states.push((
            format!("{} + {lang} + {}", platforms[0], lang_depot.id),
            Some(platforms[0].clone()),
            Some(lang),
            owned,
        ));
    }
    for dlc in app_meta
        .depots
        .iter()
        .filter(|d| d.optional && d.language.is_none())
        .take(2)
    {
        let mut owned = base_owned.clone();
        owned.push(dlc.id.clone());
        states.push((
            format!("{} + base + {}", platforms[0], dlc.id),
            Some(platforms[0].clone()),
            None,
            owned,
        ));
    }
    for (label, platform, language, owned) in &states {
        let file = plans_dir.join(format!(
            "{}.md",
            label.replace([' ', '+', '/'], "-").replace("--", "-")
        ));
        commands.push(format!(
            "cavs install-plan --workspace {} --app {app_id}{}{} --owned {}{} --to {}",
            ws_path.display(),
            platform
                .as_ref()
                .map(|p| format!(" --platform {p}"))
                .unwrap_or_default(),
            language
                .as_ref()
                .map(|l| format!(" --language {l}"))
                .unwrap_or_default(),
            owned.join(","),
            from.map(|f| format!(" --from {f}")).unwrap_or_default(),
            to_build.id
        ));
        let res = workspace_cmd::install_plan(&InstallPlanArgs {
            workspace: ws_path,
            app: Some(&app_id),
            branch: None,
            platform: platform.as_deref(),
            language: language.as_deref(),
            owned: owned.clone(),
            from_build: from_build.as_ref().map(|b| b.id.as_str()),
            to_build: Some(&to_build.id),
            json: false,
            out: Some(&file),
        });
        rows.push(match &res {
            Ok(()) => CheckRow::new(
                &format!("install-plan {label}"),
                CheckResult::Pass,
                format!(
                    "install-plans/{}",
                    file.file_name().unwrap().to_string_lossy()
                ),
            ),
            Err(e) => CheckRow::new(
                &format!("install-plan {label}"),
                CheckResult::Fail,
                format!("{e:#}"),
            ),
        });
    }

    let result = worst(&rows);
    Ok(Outcome {
        rows,
        result,
        depot_pairs,
        depot_costs,
    })
}

#[derive(serde::Serialize)]
struct Report<'a> {
    schema: &'static str,
    result: CheckResult,
    checks: &'a [CheckRow],
    depot_costs: Vec<serde_json::Value>,
}

pub fn write_reports(outcome: &Outcome, out_dir: &Path) -> Result<()> {
    std::fs::write(
        out_dir.join("workspace.json"),
        serde_json::to_vec_pretty(&Report {
            schema: "cavs-certify-workspace/1",
            result: outcome.result,
            checks: &outcome.rows,
            depot_costs: outcome
                .depot_costs
                .iter()
                .map(|(d, cost, total)| {
                    serde_json::json!({
                        "depot": d,
                        "update_bytes": cost,
                        "total_bytes": total,
                    })
                })
                .collect(),
        })?,
    )?;
    let mut md = String::from("# Workspace Certification\n\n");
    md.push_str(&format!("Result: **{}**\n\n", outcome.result.label()));
    md.push_str(&super::rows_markdown(&outcome.rows));
    if !outcome.depot_costs.is_empty() {
        md.push_str(
            "\n## Per-depot update cost\n\n| Depot | Update | Depot total |\n|---|---:|---:|\n",
        );
        for (d, cost, total) in &outcome.depot_costs {
            md.push_str(&format!(
                "| {d} | {} | {} |\n",
                human_bytes(*cost),
                human_bytes(*total)
            ));
        }
    }
    std::fs::write(out_dir.join("workspace.md"), md)?;
    Ok(())
}
