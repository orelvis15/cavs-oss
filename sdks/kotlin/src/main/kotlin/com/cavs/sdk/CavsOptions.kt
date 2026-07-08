package com.cavs.sdk

import com.cavs.sdk.nativebridge.FfmNativeBridge
import com.cavs.sdk.nativebridge.NativeBridge

/** Construction options for a [CavsClient]. */
class CavsOptions private constructor(internal val bridge: NativeBridge) {

    class Builder {
        private var bridge: NativeBridge? = null

        /** Override the native backend (e.g. a test double). */
        fun bridge(bridge: NativeBridge) = apply { this.bridge = bridge }

        fun build() = CavsOptions(bridge ?: FfmNativeBridge())
    }

    companion object {
        /** Defaults: the FFM native bridge. */
        fun defaults() = CavsOptions(FfmNativeBridge())

        fun builder() = Builder()
    }
}
