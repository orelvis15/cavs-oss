package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.List;

/** Per-category verification mismatches. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record Mismatches(
        List<String> modified,
        List<String> missing,
        List<String> extra) {
}
