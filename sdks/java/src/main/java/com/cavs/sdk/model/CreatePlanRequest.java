package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code createPlan}. Provide {@code oldPath} or {@code oldSignature}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record CreatePlanRequest(
        String oldPath,
        String oldSignature,
        String newPath,
        String outputPlan,
        String planKind,
        Integer blockKib,
        Integer zstdLevel) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String oldPath;
        private String oldSignature;
        private String newPath;
        private String outputPlan;
        private String planKind;
        private Integer blockKib;
        private Integer zstdLevel;

        public Builder oldPath(String v) { this.oldPath = v; return this; }
        public Builder oldSignature(String v) { this.oldSignature = v; return this; }
        public Builder newPath(String v) { this.newPath = v; return this; }
        public Builder outputPlan(String v) { this.outputPlan = v; return this; }
        public Builder planKind(String v) { this.planKind = v; return this; }
        public Builder blockKib(int v) { this.blockKib = v; return this; }
        public Builder zstdLevel(int v) { this.zstdLevel = v; return this; }

        public CreatePlanRequest build() {
            return new CreatePlanRequest(oldPath, oldSignature, newPath, outputPlan, planKind, blockKib, zstdLevel);
        }
    }
}
