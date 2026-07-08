package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;
import java.util.List;

/** Request for {@code previewUpdate} / {@code compareRoutes}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record PreviewRequest(
        String oldPath,
        String newPath,
        String engineHint,
        List<String> routes,
        String policy) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String oldPath;
        private String newPath;
        private String engineHint;
        private List<String> routes;
        private String policy;

        public Builder oldPath(String v) { this.oldPath = v; return this; }
        public Builder newPath(String v) { this.newPath = v; return this; }
        public Builder engineHint(String v) { this.engineHint = v; return this; }
        public Builder routes(List<String> v) { this.routes = v; return this; }
        public Builder policy(String v) { this.policy = v; return this; }

        public PreviewRequest build() {
            return new PreviewRequest(oldPath, newPath, engineHint, routes, policy);
        }
    }
}
