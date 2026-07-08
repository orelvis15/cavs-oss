package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** Result of {@code packDirectory}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record PackResult(
        String outputCavs,
        long totalSizeBytes,
        long chunkCount,
        long logicalChunks,
        long logicalRawBytes,
        long storedBytes,
        String merkleRoot,
        long filesPacked,
        long entriesIgnored,
        boolean signed,
        String profile,
        long elapsedMs) {
}
