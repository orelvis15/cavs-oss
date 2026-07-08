package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;
import java.util.List;

/** Request for {@code packDirectory}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record PackDirectoryRequest(
        String inputDir,
        String outputCavs,
        String profile,
        String compression,
        String signKeyPath,
        List<String> ignore) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private String inputDir;
        private String outputCavs;
        private String profile;
        private String compression;
        private String signKeyPath;
        private List<String> ignore;

        public Builder inputDir(String v) { this.inputDir = v; return this; }
        public Builder outputCavs(String v) { this.outputCavs = v; return this; }
        public Builder profile(String v) { this.profile = v; return this; }
        public Builder compression(String v) { this.compression = v; return this; }
        public Builder signKeyPath(String v) { this.signKeyPath = v; return this; }
        public Builder ignore(List<String> v) { this.ignore = v; return this; }

        public PackDirectoryRequest build() {
            return new PackDirectoryRequest(inputDir, outputCavs, profile, compression, signKeyPath, ignore);
        }
    }
}
