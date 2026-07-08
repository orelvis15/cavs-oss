package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** Result of {@code applyPlan}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record ApplyResult(
        String outputPath,
        boolean verified,
        String mode,
        long filesTotal,
        long filesWritten,
        long filesNoop,
        long dirsCreated,
        long symlinksCreated,
        long deleted,
        long bytesWritten,
        long bytesFromOld,
        long bytesFromBlob,
        long elapsedMs) {
}
