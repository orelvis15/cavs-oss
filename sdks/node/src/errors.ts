/** A CAVS operation failure carrying the stable engine error code. */
export class CavsError extends Error {
  readonly code: string;
  readonly recoverable: boolean;
  readonly details: Record<string, unknown>;

  constructor(
    code: string,
    message: string,
    recoverable = false,
    details: Record<string, unknown> = {},
  ) {
    super(message);
    this.name = "CavsError";
    this.code = code;
    this.recoverable = recoverable;
    this.details = details;
  }
}

/** Well-known error codes (the engine defines the full set). */
export const ErrorCode = {
  PathNotFound: "CAVS-E-PATH-NOT-FOUND",
  PathTraversal: "CAVS-E-PATH-TRAVERSAL",
  InvalidRequest: "CAVS-E-INVALID-REQUEST",
  UnknownOperation: "CAVS-E-UNKNOWN-OPERATION",
  Cancelled: "CAVS-E-CANCELLED",
} as const;
