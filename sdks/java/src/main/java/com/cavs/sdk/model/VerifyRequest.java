package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code verifyInstall}. Provide {@code signature} or {@code manifest}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record VerifyRequest(
        String target,
        String signature,
        String manifest,
        Boolean allowExtra) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String target;
        private String signature;
        private String manifest;
        private Boolean allowExtra;

        public Builder target(String v) { this.target = v; return this; }
        public Builder signature(String v) { this.signature = v; return this; }
        public Builder manifest(String v) { this.manifest = v; return this; }
        public Builder allowExtra(boolean v) { this.allowExtra = v; return this; }

        public VerifyRequest build() {
            return new VerifyRequest(target, signature, manifest, allowExtra);
        }
    }
}
