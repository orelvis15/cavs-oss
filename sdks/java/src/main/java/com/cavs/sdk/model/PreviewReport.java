package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.List;

/** Result of {@code previewUpdate} / {@code compareRoutes}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record PreviewReport(
        String recommendedRoute,
        long oldSizeBytes,
        long newSizeBytes,
        List<Route> routes,
        String explanation) {
}
