package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.List;

/** Result of {@code analyze}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record AnalyzeReport(
        AnalyzeSummary summary,
        String engine,
        List<String> warnings,
        List<Recommendation> recommendations,
        String note) {
}
