package com.cavs.sdk.nativebridge;

import java.util.function.Consumer;

/**
 * The seam between the Java SDK and the native CAVS library. Handles are
 * opaque {@code long} addresses. The shipped implementation is
 * {@link FfmNativeBridge} (Java Foreign Function &amp; Memory API, JEP 454); a
 * JNA-based backend for the Java 17 baseline can be added behind this same
 * interface without touching {@code CavsClient}.
 */
public interface NativeBridge {

    String version();

    String abiVersion();

    String capabilitiesJson();

    long contextNew();

    void contextFree(long ctx);

    /** Register (or clear, with {@code null}) the progress sink for a context. */
    void setProgressCallback(long ctx, Consumer<String> sink);

    /** Run synchronously; returns the response envelope JSON. */
    String executeSync(long ctx, String operation, String requestJson);

    /** Start a background job; returns its handle (0 on rejection). */
    long startJob(long ctx, String operation, String requestJson);

    /** Poll a job: the envelope JSON if finished, else {@code null}. */
    String pollJob(long job);

    void cancelJob(long job);

    void freeJob(long job);
}
