package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code benchmark}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record BenchmarkRequest(
        String oldPath,
        String newPath,
        String engineHint,
        Boolean measureApply) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String oldPath;
        private String newPath;
        private String engineHint;
        private Boolean measureApply;

        public Builder oldPath(String v) { this.oldPath = v; return this; }
        public Builder newPath(String v) { this.newPath = v; return this; }
        public Builder engineHint(String v) { this.engineHint = v; return this; }
        public Builder measureApply(boolean v) { this.measureApply = v; return this; }

        public BenchmarkRequest build() {
            return new BenchmarkRequest(oldPath, newPath, engineHint, measureApply);
        }
    }
}
