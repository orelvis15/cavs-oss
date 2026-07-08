package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** A changed file ranked by update cost. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record WorstFile(
        String path,
        String status,
        boolean isPack,
        long oldSizeBytes,
        long newSizeBytes,
        long estimatedDownloadBytes,
        double reuseRatio,
        double entropyBits) {
}
