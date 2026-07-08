package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** Result of {@code createPlan}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record PlanResult(
        String planPath,
        long planBytes,
        String planKind,
        String mode,
        long operationCount,
        long copyOps,
        long inlineOps,
        long reusedBytes,
        long inlineBytes,
        long estimatedNetworkBytes,
        long expectedOutputSize,
        long files,
        long unchangedFiles,
        long deleted,
        long elapsedMs) {
}
