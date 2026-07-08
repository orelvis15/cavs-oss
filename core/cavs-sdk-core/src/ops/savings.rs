//! `estimateSavings` — pure arithmetic over a pricing model. Given the
//! per-download bytes of a full download vs a CAVS update and the monthly
//! download volume, return the monthly egress cost of each and the savings.

use crate::error::{Result, SdkError};
use crate::progress::OpCtx;
use serde::Deserialize;
use serde_json::{json, Value};

const BYTES_PER_GB: f64 = 1_073_741_824.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavingsRequest {
    price_per_gb: f64,
    monthly_downloads: f64,
    average_full_download_bytes: f64,
    average_cavs_download_bytes: f64,
}

pub fn run(_ctx: &OpCtx, request: &Value) -> Result<Value> {
    let req: SavingsRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    if req.price_per_gb < 0.0 || req.monthly_downloads < 0.0 {
        return Err(SdkError::InvalidRequest(
            "pricePerGb and monthlyDownloads must be non-negative".to_string(),
        ));
    }

    let full_gb = req.average_full_download_bytes / BYTES_PER_GB;
    let cavs_gb = req.average_cavs_download_bytes / BYTES_PER_GB;
    let full_cost = full_gb * req.monthly_downloads * req.price_per_gb;
    let cavs_cost = cavs_gb * req.monthly_downloads * req.price_per_gb;
    let savings = full_cost - cavs_cost;
    let savings_percent = if full_cost > 0.0 {
        savings / full_cost * 100.0
    } else {
        0.0
    };

    let round2 = |v: f64| (v * 100.0).round() / 100.0;
    let round1 = |v: f64| (v * 10.0).round() / 10.0;

    Ok(json!({
        "fullDownloadMonthlyCost": round2(full_cost),
        "cavsMonthlyCost": round2(cavs_cost),
        "estimatedMonthlySavings": round2(savings),
        "savingsPercent": round1(savings_percent),
    }))
}
