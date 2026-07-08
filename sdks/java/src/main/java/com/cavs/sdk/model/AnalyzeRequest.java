package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code analyze}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record AnalyzeRequest(
        String oldPath,
        String newPath,
        String engineHint,
        Integer maxWorstFiles) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String oldPath;
        private String newPath;
        private String engineHint;
        private Integer maxWorstFiles;

        public Builder oldPath(String v) { this.oldPath = v; return this; }
        public Builder newPath(String v) { this.newPath = v; return this; }
        public Builder engineHint(String v) { this.engineHint = v; return this; }
        public Builder maxWorstFiles(int v) { this.maxWorstFiles = v; return this; }

        public AnalyzeRequest build() {
            return new AnalyzeRequest(oldPath, newPath, engineHint, maxWorstFiles);
        }
    }
}
