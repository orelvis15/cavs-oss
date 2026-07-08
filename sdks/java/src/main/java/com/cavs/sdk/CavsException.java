package com.cavs.sdk;

import java.util.Map;

/** Thrown when a CAVS operation fails; carries the stable engine error code. */
public final class CavsException extends RuntimeException {

    private final CavsErrorCode code;
    private final String wireCode;
    private final boolean recoverable;
    private final transient Map<String, Object> details;

    public CavsException(String wireCode, String message, boolean recoverable, Map<String, Object> details) {
        super(message);
        this.wireCode = wireCode;
        this.code = CavsErrorCode.fromWire(wireCode);
        this.recoverable = recoverable;
        this.details = details == null ? Map.of() : Map.copyOf(details);
    }

    /** The parsed error code. */
    public CavsErrorCode code() {
        return code;
    }

    /** The raw wire code (useful when the engine adds codes this SDK predates). */
    public String wireCode() {
        return wireCode;
    }

    /** Whether retrying the same request could plausibly succeed. */
    public boolean recoverable() {
        return recoverable;
    }

    /** Structured, code-specific detail fields (may be empty). */
    public Map<String, Object> details() {
        return details;
    }
}
