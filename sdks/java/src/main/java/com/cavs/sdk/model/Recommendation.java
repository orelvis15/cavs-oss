package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** A detector finding with its recommended fix. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record Recommendation(
        String severity,
        String kind,
        String title,
        String file,
        long estimatedWastedBytes,
        String why,
        String fix,
        String expectedImprovement) {
}
