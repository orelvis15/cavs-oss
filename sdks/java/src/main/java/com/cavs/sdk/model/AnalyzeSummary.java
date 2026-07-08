package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.List;

/** Headline numbers for an old→new build transition. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record AnalyzeSummary(
        long oldSizeBytes,
        long newSizeBytes,
        long estimatedUpdateBytes,
        long estimatedSteamPipeBytes,
        double cavsReuseRatio,
        double steamPipeReuseRatio,
        int filesUnchanged,
        int filesModified,
        int filesAdded,
        int filesDeleted,
        List<WorstFile> worstFiles) {
}
