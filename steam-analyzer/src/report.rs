//! Report writers: Markdown, JSON, CSV and a self-contained HTML page.

use crate::analyze::{FileDiff, Report};
use std::path::Path;

pub fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {}", U[i])
    }
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

pub fn write_all(report: &Report, out_dir: &Path, formats: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    for f in formats {
        match f.as_str() {
            "json" => std::fs::write(out_dir.join("results.json"), json(report)?)?,
            "md" => std::fs::write(out_dir.join("summary.md"), markdown(report))?,
            "csv" => std::fs::write(out_dir.join("files.csv"), csv(report))?,
            "html" => std::fs::write(out_dir.join("index.html"), html(report))?,
            other => eprintln!("[warn] unknown report format: {other}"),
        }
    }
    Ok(())
}

pub fn json(report: &Report) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}

pub fn markdown(r: &Report) -> String {
    let mut m = String::new();
    m.push_str("# Steam Update Risk Report\n\n");
    m.push_str(&format!("_{}_\n\n", r.note));
    m.push_str("## Summary\n\n");
    m.push_str(&format!("- Old build: {}\n", human(r.old_size_bytes)));
    m.push_str(&format!("- New build: {}\n", human(r.new_size_bytes)));
    m.push_str(&format!(
        "- Changed files: {} · new files: {}\n",
        r.changed_files, r.new_files
    ));
    m.push_str(&format!(
        "- **Estimated SteamPipe update: {}** (raw {}, reuse {})\n",
        human(r.estimated_steam_update_bytes),
        human(r.estimated_steam_update_raw_bytes),
        pct(r.steam_reuse_ratio)
    ));
    m.push_str(&format!(
        "- CAVS FastCDC 64 KiB estimate: {} (reuse {})\n",
        human(r.estimated_cdc_update_bytes),
        pct(r.cdc_reuse_ratio)
    ));
    m.push_str(&format!("- **Risk: {}**\n\n", r.risk.label().to_uppercase()));

    m.push_str("## Top offenders\n\n");
    m.push_str("| # | File | Size | Steam update | CDC update | Risk | Reasons |\n");
    m.push_str("|--:|---|--:|--:|--:|---|---|\n");
    for (i, d) in r.top_offenders.iter().enumerate() {
        m.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            i + 1,
            d.path,
            human(d.new_size),
            human(d.steam_payload_compressed),
            human(d.cdc_payload),
            d.risk.label(),
            d.reasons.join(", ")
        ));
    }

    m.push_str("\n## Steam-like vs CAVS estimate\n\n");
    m.push_str("| Model | Estimated update | Reuse |\n|---|--:|--:|\n");
    m.push_str(&format!(
        "| SteamPipe fixed 1 MiB | {} | {} |\n",
        human(r.estimated_steam_update_bytes),
        pct(r.steam_reuse_ratio)
    ));
    m.push_str(&format!(
        "| CAVS FastCDC 64 KiB | {} | {} |\n",
        human(r.estimated_cdc_update_bytes),
        pct(r.cdc_reuse_ratio)
    ));

    m.push_str("\n## Recommendations\n\n");
    for rec in &r.recommendations {
        m.push_str(&format!(
            "- **[{}] {}** — {}\n",
            rec.severity.to_uppercase(),
            rec.title,
            rec.detail
        ));
    }
    m
}

pub fn csv(r: &Report) -> String {
    let mut s = String::from(
        "path,status,is_pack,old_size,new_size,steam_update_bytes,cdc_update_bytes,\
         steam_reuse,cdc_reuse,changed_window_ratio,risk,reasons\n",
    );
    for d in &r.top_offenders {
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{},{}\n",
            d.path,
            d.status,
            d.is_pack,
            d.old_size,
            d.new_size,
            d.steam_payload_compressed,
            d.cdc_payload,
            d.steam_reuse_ratio,
            d.cdc_reuse_ratio,
            d.changed_window_ratio,
            d.risk.label(),
            d.reasons.join("|")
        ));
    }
    s
}

fn risk_color(level: &str) -> &'static str {
    match level {
        "high" => "#ff6b6b",
        "medium" => "#ffb454",
        "low" => "#5df2a6",
        _ => "#8fa89a",
    }
}

pub fn html(r: &Report) -> String {
    let offenders: String = r
        .top_offenders
        .iter()
        .enumerate()
        .map(|(i, d)| offender_row(i + 1, d))
        .collect();
    let recs: String = r
        .recommendations
        .iter()
        .map(|rec| {
            format!(
                "<div class=co style=\"border-left-color:{}\"><b>[{}] {}</b><p>{}</p></div>",
                risk_color(&rec.severity),
                rec.severity.to_uppercase(),
                esc(&rec.title),
                esc(&rec.detail)
            )
        })
        .collect();
    let rc = risk_color(r.risk.label());
    format!(
        r##"<!doctype html><html lang=en><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>CAVS SteamPipe Analyzer — Report</title><style>
:root{{color-scheme:dark}}*{{box-sizing:border-box;margin:0}}
body{{background:#0a0d0b;color:#e8f2ec;font:15px/1.55 system-ui,-apple-system,sans-serif;
padding:2rem 1.5rem 4rem;max-width:1040px;margin:0 auto}}
h1{{font:800 clamp(26px,4vw,40px)/1.05 "Avenir Next Condensed","Arial Narrow",sans-serif;letter-spacing:.02em}}
h1 span{{color:#5df2a6}} h2{{font-size:1.15rem;margin:2rem 0 .6rem;color:#5df2a6}}
.note{{color:#5a6f63;font:12px/1.5 ui-monospace,monospace;margin:.4rem 0 1.4rem}}
.cards{{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:1px;background:#1e2a23;border:1px solid #1e2a23;margin:1rem 0}}
.card{{background:#101613;padding:18px 20px}}.card b{{display:block;font:800 clamp(24px,3vw,34px)/1 "Avenir Next Condensed",sans-serif}}
.card small{{color:#8fa89a;font:11px/1.5 ui-monospace,monospace;display:block;margin-top:8px}}
.risk{{display:inline-block;padding:6px 14px;border-radius:3px;font:700 13px/1 ui-monospace,monospace;letter-spacing:.1em;color:#0a0d0b;background:{rc}}}
table{{width:100%;border-collapse:collapse;font:12px/1.5 ui-monospace,monospace;margin:1rem 0}}
th,td{{border:1px solid #1e2a23;padding:8px 10px;text-align:right}}
th:nth-child(2),td:nth-child(2),th:last-child,td:last-child{{text-align:left}}
th{{background:#101613;color:#8fa89a;text-transform:uppercase;font-size:10px}}
td{{color:#8fa89a}} td.f{{color:#e8f2ec}} .pill{{padding:2px 7px;border-radius:2px;font-weight:700}}
.co{{background:#101613;border:1px solid #1e2a23;border-left:3px solid #5df2a6;padding:14px 16px;margin:10px 0}}
.co p{{color:#8fa89a;font-size:13px;margin-top:6px}}
.bars{{margin:1rem 0}}.bar-row{{display:grid;grid-template-columns:200px 1fr 120px;gap:10px;align-items:center;margin:6px 0;font:12px/1 ui-monospace,monospace}}
.bar-row span:first-child{{color:#8fa89a;text-align:right}}.track{{height:20px;background:#0d120f;border:1px solid #1e2a23}}
.fill{{height:100%}}.fill.s{{background:repeating-linear-gradient(-45deg,#ffb45440 0 6px,#ffb45420 6px 12px);border-right:2px solid #ffb454}}
.fill.c{{background:linear-gradient(90deg,#5df2a640,#5df2a620);border-right:2px solid #5df2a6}}
</style></head><body>
<h1>CAVS <span>SteamPipe</span> Analyzer</h1>
<div class=note>// {note}</div>
<h2>Summary</h2>
<div class=cards>
<div class=card><b>{steam}</b><small>estimated SteamPipe update</small></div>
<div class=card><b>{cdc}</b><small>CAVS FastCDC 64 KiB update</small></div>
<div class=card><b>{changed}</b><small>changed + {new} new files</small></div>
<div class=card><b>{newb}</b><small>new build size ({oldb} old)</small></div>
</div>
<p><span class=risk>RISK: {risklabel}</span></p>
<div class=bars>
<div class=bar-row><span>SteamPipe 1 MiB</span><div class=track><div class="fill s" style="width:{sw}%"></div></div><span>{steam}</span></div>
<div class=bar-row><span>CAVS 64 KiB</span><div class=track><div class="fill c" style="width:{cw}%"></div></div><span>{cdc}</span></div>
</div>
<h2>Top offenders</h2>
<table><tr><th>#</th><th>file</th><th>size</th><th>steam update</th><th>cdc update</th><th>risk</th><th>reasons</th></tr>{offenders}</table>
<h2>Recommendations</h2>{recs}
</body></html>"##,
        rc = rc,
        note = esc(&r.note),
        steam = human(r.estimated_steam_update_bytes),
        cdc = human(r.estimated_cdc_update_bytes),
        changed = r.changed_files,
        new = r.new_files,
        newb = human(r.new_size_bytes),
        oldb = human(r.old_size_bytes),
        risklabel = r.risk.label().to_uppercase(),
        sw = bar_width(r.estimated_steam_update_bytes, r.estimated_steam_update_bytes),
        cw = bar_width(r.estimated_cdc_update_bytes, r.estimated_steam_update_bytes),
        offenders = offenders,
        recs = recs,
    )
}

fn bar_width(part: u64, max: u64) -> f64 {
    if max == 0 {
        0.0
    } else {
        (part as f64 / max as f64 * 100.0).clamp(0.4, 100.0)
    }
}

fn offender_row(rank: usize, d: &FileDiff) -> String {
    format!(
        "<tr><td>{}</td><td class=f>{}</td><td>{}</td><td>{}</td><td>{}</td>\
         <td><span class=pill style=\"color:{c}\">{}</span></td><td>{}</td></tr>",
        rank,
        esc(&d.path),
        human(d.new_size),
        human(d.steam_payload_compressed),
        human(d.cdc_payload),
        d.risk.label(),
        esc(&d.reasons.join(", ")),
        c = risk_color(d.risk.label()),
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
