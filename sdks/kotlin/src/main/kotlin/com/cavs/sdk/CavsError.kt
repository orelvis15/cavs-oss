package com.cavs.sdk

/** Stable CAVS engine error codes, mirrored from the Rust `CAVS-E-*` set. */
enum class CavsErrorCode(val wire: String) {
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

    companion object {
        /** Map a wire code to an enum, defaulting to [UNKNOWN]. */
        fun fromWire(code: String?): CavsErrorCode =
            entries.firstOrNull { it.wire == code } ?: UNKNOWN
    }
}

/**
 * Thrown when a CAVS operation fails; carries the stable engine error code.
 *
 * @property code the parsed error code
 * @property wireCode the raw wire code (useful when the engine adds codes
 *   this SDK predates)
 * @property recoverable whether retrying the same request could succeed
 * @property details structured, code-specific detail fields
 */
class CavsException(
    val wireCode: String,
    message: String,
    val recoverable: Boolean = false,
    val details: Map<String, Any?> = emptyMap(),
) : RuntimeException(message) {
    val code: CavsErrorCode = CavsErrorCode.fromWire(wireCode)
}
