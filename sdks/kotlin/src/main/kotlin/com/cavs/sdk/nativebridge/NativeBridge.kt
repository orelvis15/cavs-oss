package com.cavs.sdk.nativebridge

/**
 * The seam between the Kotlin SDK and the native CAVS library. Handles are
 * opaque `Long` addresses. The shipped implementation is [FfmNativeBridge]
 * (Java Foreign Function & Memory API, JEP 454); an alternative backend can
 * slot in behind this interface without touching `CavsClient`.
 */
interface NativeBridge {

    fun version(): String

    fun abiVersion(): String

    fun capabilitiesJson(): String

    fun contextNew(): Long

    fun contextFree(ctx: Long)

    /** Register (or clear, with `null`) the progress sink for a context. */
    fun setProgressCallback(ctx: Long, sink: ((String) -> Unit)?)

    /** Run synchronously; returns the response envelope JSON. */
    fun executeSync(ctx: Long, operation: String, requestJson: String): String

    /** Start a background job; returns its handle (0 on rejection). */
    fun startJob(ctx: Long, operation: String, requestJson: String): Long

    /** Poll a job: the envelope JSON if finished, else `null`. */
    fun pollJob(job: Long): String?

    fun cancelJob(job: Long)

    fun freeJob(job: Long)
}
