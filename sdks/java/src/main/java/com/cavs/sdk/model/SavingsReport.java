package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** Result of {@code estimateSavings}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record SavingsReport(
        double fullDownloadMonthlyCost,
        double cavsMonthlyCost,
        double estimatedMonthlySavings,
        double savingsPercent) {
}
