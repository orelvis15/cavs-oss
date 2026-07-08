package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;

/** One progress event emitted during a long-running operation. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record ProgressEvent(
        @JsonProperty("type") String type,
        String operation,
        String phase,
        long currentBytes,
        long totalBytes,
        Double percentage,
        String message) {
}
