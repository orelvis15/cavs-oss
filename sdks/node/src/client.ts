import { native, Pointer } from "./native";
import { CavsError, ErrorCode } from "./errors";
import type {
  AnalyzeReport,
  AnalyzeRequest,
  ApplyPlanRequest,
  ApplyResult,
  BenchmarkReport,
  BenchmarkRequest,
  CallOptions,
  CreatePlanRequest,
  PackDirectoryRequest,
  PackResult,
  PlanResult,
  PreviewReport,
  PreviewRequest,
  ProgressEvent,
  SavingsReport,
  SavingsRequest,
  VerifyRequest,
  VerifyResult,
} from "./types";

const SCHEMA_VERSION = "1.0";
const POLL_INTERVAL_MS = 1;

interface Envelope {
  ok: boolean;
  data?: unknown;
  error?: { code: string; message: string; recoverable?: boolean; details?: Record<string, unknown> };
}

/** Version of the native SDK. */
export function version(): string {
  return native.version();
}

/** Native C ABI contract version. */
export function abiVersion(): string {
  return native.abiVersion();
}

/**
 * A CAVS client. Owns a native context; call {@link close} when done.
 *
 * Calls are serialized (a single native context carries one progress
 * callback slot), so concurrent calls on one client run one at a time —
 * create multiple clients for parallelism.
 */
export class CavsClient {
  private ctx: Pointer;
  private closed = false;
  private queue: Promise<unknown> = Promise.resolve();

  constructor() {
    this.ctx = native.contextNew();
    if (!this.ctx) {
      throw new Error("cavs: failed to create native context");
    }
  }

  close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    native.contextFree(this.ctx);
  }

  analyze(req: AnalyzeRequest, opts?: CallOptions): Promise<AnalyzeReport> {
    return this.call("analyze", req, opts);
  }

  packDirectory(req: PackDirectoryRequest, opts?: CallOptions): Promise<PackResult> {
    return this.call("packDirectory", req, opts);
  }

  preview(req: PreviewRequest, opts?: CallOptions): Promise<PreviewReport> {
    return this.call("previewUpdate", req, opts);
  }

  createPlan(req: CreatePlanRequest, opts?: CallOptions): Promise<PlanResult> {
    return this.call("createPlan", req, opts);
  }

  applyPlan(req: ApplyPlanRequest, opts?: CallOptions): Promise<ApplyResult> {
    return this.call("applyPlan", req, opts);
  }

  verifyInstall(req: VerifyRequest, opts?: CallOptions): Promise<VerifyResult> {
    return this.call("verifyInstall", req, opts);
  }

  benchmark(req: BenchmarkRequest, opts?: CallOptions): Promise<BenchmarkReport> {
    return this.call("benchmark", req, opts);
  }

  estimateSavings(req: SavingsRequest, opts?: CallOptions): Promise<SavingsReport> {
    return this.call("estimateSavings", req, opts);
  }

  /** Serialize calls onto one native context. */
  private call<T>(op: string, req: unknown, opts?: CallOptions): Promise<T> {
    const run = this.queue.then(() => this.execute<T>(op, req, opts));
    // Keep the chain alive even if a call rejects.
    this.queue = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  private async execute<T>(op: string, req: unknown, opts?: CallOptions): Promise<T> {
    if (this.closed) {
      throw new CavsError(ErrorCode.InvalidRequest, "client is closed");
    }
    const body = JSON.stringify({ schemaVersion: SCHEMA_VERSION, data: req });

    if (opts?.onProgress) {
      return this.executeWithProgress<T>(op, body, opts.onProgress);
    }
    return this.executeAsJob<T>(op, body, opts?.signal);
  }

  /** Progress path: synchronous native call so callbacks fire on this
   *  thread (koffi cannot invoke JS callbacks from a foreign thread). */
  private executeWithProgress<T>(op: string, body: string, onProgress: (e: ProgressEvent) => void): T {
    const cbPtr = native.registerProgress((raw) => {
      try {
        onProgress(JSON.parse(raw) as ProgressEvent);
      } catch {
        /* a malformed event must not break the operation */
      }
    });
    try {
      native.setProgress(this.ctx, cbPtr);
      const envelope = native.executeSync(this.ctx, op, body);
      return parse<T>(envelope);
    } finally {
      native.setProgress(this.ctx, null);
      native.unregister(cbPtr);
    }
  }

  /** Non-blocking path: run on a native worker thread, poll on a timer so
   *  the event loop stays free; AbortSignal cancels the native job. */
  private async executeAsJob<T>(op: string, body: string, signal?: AbortSignal): Promise<T> {
    if (signal?.aborted) {
      throw new CavsError(ErrorCode.Cancelled, "operation aborted before start", true);
    }
    const job = native.startJob(this.ctx, op, body);
    if (!job) {
      throw new CavsError(ErrorCode.InvalidRequest, "native library rejected the request");
    }
    let cancelled = false;
    try {
      for (;;) {
        const envelope = native.pollJob(job);
        if (envelope !== null) {
          if (cancelled) {
            throw new CavsError(ErrorCode.Cancelled, "operation aborted", true);
          }
          return parse<T>(envelope);
        }
        if (signal?.aborted && !cancelled) {
          native.cancelJob(job);
          cancelled = true;
        }
        await sleep(POLL_INTERVAL_MS);
      }
    } finally {
      native.freeJob(job);
    }
  }
}

function parse<T>(envelope: string): T {
  const env = JSON.parse(envelope) as Envelope;
  if (!env.ok) {
    const e = env.error;
    throw new CavsError(
      e?.code ?? "CAVS-E-UNKNOWN",
      e?.message ?? "operation failed",
      e?.recoverable ?? false,
      e?.details ?? {},
    );
  }
  return env.data as T;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
