package com.cavs.sdk;

import com.cavs.sdk.nativebridge.FfmNativeBridge;
import com.cavs.sdk.nativebridge.NativeBridge;

/** Construction options for a {@link CavsClient}. */
public final class CavsOptions {

    private final NativeBridge bridge;

    private CavsOptions(NativeBridge bridge) {
        this.bridge = bridge;
    }

    /** Defaults: the FFM native bridge. */
    public static CavsOptions defaults() {
        return new CavsOptions(new FfmNativeBridge());
    }

    public static Builder builder() {
        return new Builder();
    }

    NativeBridge bridge() {
        return bridge;
    }

    public static final class Builder {
        private NativeBridge bridge;

        /** Override the native backend (e.g. a JNA bridge or a test double). */
        public Builder bridge(NativeBridge b) {
            this.bridge = b;
            return this;
        }

        public CavsOptions build() {
            return new CavsOptions(bridge != null ? bridge : new FfmNativeBridge());
        }
    }
}
