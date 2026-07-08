package com.cavs.sdk.model;

import com.fasterxml.jackson.annotation.JsonInclude;

/** Request for {@code estimateSavings}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public record SavingsRequest(
        double pricePerGb,
        double monthlyDownloads,
        double averageFullDownloadBytes,
        double averageCavsDownloadBytes) {

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private double pricePerGb;
        private double monthlyDownloads;
        private double averageFullDownloadBytes;
        private double averageCavsDownloadBytes;

        public Builder pricePerGb(double v) { this.pricePerGb = v; return this; }
        public Builder monthlyDownloads(double v) { this.monthlyDownloads = v; return this; }
        public Builder averageFullDownloadBytes(double v) { this.averageFullDownloadBytes = v; return this; }
        public Builder averageCavsDownloadBytes(double v) { this.averageCavsDownloadBytes = v; return this; }

        public SavingsRequest build() {
            return new SavingsRequest(pricePerGb, monthlyDownloads, averageFullDownloadBytes, averageCavsDownloadBytes);
        }
    }
}
