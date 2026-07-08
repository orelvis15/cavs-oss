package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.List;

/** Result of {@code benchmark}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record BenchmarkReport(
        long oldSizeBytes,
        long newSizeBytes,
        String recommendedRoute,
        List<Route> routes,
        double reuseRatio) {
}
