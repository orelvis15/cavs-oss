package com.cavs.sdk;

/** Stable CAVS engine error codes, mirrored from the Rust {@code CAVS-E-*} set. */
public enum CavsErrorCode {
    PATH_NOT_FOUND("CAVS-E-PATH-NOT-FOUND"),
    PATH_TRAVERSAL("CAVS-E-PATH-TRAVERSAL"),
    INVALID_REQUEST("CAVS-E-INVALID-REQUEST"),
    INVALID_JSON("CAVS-E-INVALID-JSON"),
    UNKNOWN_OPERATION("CAVS-E-UNKNOWN-OPERATION"),
    UNSUPPORTED_SCHEMA("CAVS-E-UNSUPPORTED-SCHEMA"),
    CANCELLED("CAVS-E-CANCELLED"),
    PLAN("CAVS-E-PLAN"),
    SIGNATURE("CAVS-E-SIGNATURE"),
    FORMAT("CAVS-E-FORMAT"),
    IO("CAVS-E-IO"),
    INTERNAL("CAVS-E-INTERNAL"),
    UNKNOWN("CAVS-E-UNKNOWN");

    private final String wire;

    CavsErrorCode(String wire) {
        this.wire = wire;
    }

    /** The wire form, e.g. {@code CAVS-E-PATH-NOT-FOUND}. */
    public String wire() {
        return wire;
    }

    /** Map a wire code to an enum, defaulting to {@link #UNKNOWN}. */
    public static CavsErrorCode fromWire(String code) {
        if (code != null) {
            for (CavsErrorCode c : values()) {
                if (c.wire.equals(code)) {
                    return c;
                }
            }
        }
        return UNKNOWN;
    }
}
