package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code applyPlan}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record ApplyPlanRequest(
        String oldPath,
        String planPath,
        String outputPath,
        Boolean checkOld,
        Boolean deleteRemoved) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String oldPath;
        private String planPath;
        private String outputPath;
        private Boolean checkOld;
        private Boolean deleteRemoved;

        public Builder oldPath(String v) { this.oldPath = v; return this; }
        public Builder planPath(String v) { this.planPath = v; return this; }
        public Builder outputPath(String v) { this.outputPath = v; return this; }
        public Builder checkOld(boolean v) { this.checkOld = v; return this; }
        public Builder deleteRemoved(boolean v) { this.deleteRemoved = v; return this; }

        public ApplyPlanRequest build() {
            return new ApplyPlanRequest(oldPath, planPath, outputPath, checkOld, deleteRemoved);
        }
    }
}
