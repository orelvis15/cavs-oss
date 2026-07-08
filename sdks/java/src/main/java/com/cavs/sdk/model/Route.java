package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** One modeled delivery route. {@code diffMs}/{@code applyMs} are set only
 *  by {@code benchmark}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record Route(
        String name,
        long networkBytes,
        Long diffMs,
        Long applyMs,
        boolean available) {
}
