package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;

/** Result of {@code verifyInstall}. */
@JsonIgnoreProperties(ignoreUnknown = true)
public record VerifyResult(
        boolean verified,
        long filesChecked,
        long bytesChecked,
        Mismatches mismatches,
        long elapsedMs) {
}
