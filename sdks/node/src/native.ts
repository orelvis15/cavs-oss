// Native binding to the CAVS C ABI via koffi. Handles are opaque pointers.
import koffi from "koffi";
import * as fs from "node:fs";
import * as path from "node:path";

function libraryFileName(): string {
  switch (process.platform) {
    case "darwin":
      return "libcavs_sdk.dylib";
    case "win32":
      return "cavs_sdk.dll";
    default:
      return "libcavs_sdk.so";
  }
}

function osArch(): string {
  const os =
    process.platform === "darwin"
      ? "darwin"
      : process.platform === "win32"
        ? "win32"
        : "linux";
  const arch = process.arch === "x64" ? "x64" : process.arch === "arm64" ? "arm64" : process.arch;
  return `${os}-${arch}`;
}

/** Resolve the native library: CAVS_SDK_LIBRARY, then the per-platform
 *  package, then the local `native/` staging dir. */
function resolveLibraryPath(): string {
  const override = process.env.CAVS_SDK_LIBRARY;
  if (override && override.length > 0) {
    return override;
  }
  const name = libraryFileName();
  const candidates = [
    // Published per-platform package: @orelvis15/cavs-sdk-<os>-<arch>.
    tryResolvePlatformPackage(name),
    // Local dev staging (npm run native).
    path.join(__dirname, "..", "native", name),
    path.join(__dirname, "..", "..", "native", name),
  ].filter((p): p is string => !!p);
  for (const c of candidates) {
    if (fs.existsSync(c)) {
      return c;
    }
  }
  throw new Error(
    `cavs: native library not found (looked for ${name}); set CAVS_SDK_LIBRARY or run \`npm run native\``,
  );
}

function tryResolvePlatformPackage(name: string): string | undefined {
  try {
    const pkg = `@orelvis15/cavs-sdk-${osArch()}`;
    const dir = path.dirname(require.resolve(`${pkg}/package.json`));
    return path.join(dir, name);
  } catch {
    return undefined;
  }
}

const lib = koffi.load(resolveLibraryPath());

// Opaque handle pointer types.
const CavsContext = koffi.pointer("CavsContext", koffi.opaque());
const CavsResult = koffi.pointer("CavsResult", koffi.opaque());
const CavsJob = koffi.pointer("CavsJob", koffi.opaque());

// Progress callback prototype: void (*)(const char*, void*).
const ProgressProto = koffi.proto("void CavsProgressCallback(const char* event, void* user)");
const ProgressPtr = koffi.pointer(ProgressProto);

const fns = {
  version: lib.func("const char* cavs_sdk_version()"),
  abiVersion: lib.func("const char* cavs_sdk_abi_version()"),
  capabilities: lib.func("void* cavs_sdk_capabilities_json()"),
  ctxNew: lib.func("CavsContext* cavs_context_new(const char*)"),
  ctxFree: lib.func("void cavs_context_free(CavsContext*)"),
  setProgress: lib.func(
    "int cavs_context_set_progress_callback(CavsContext*, CavsProgressCallback*, void*)",
  ),
  execute: lib.func("CavsResult* cavs_execute_json(CavsContext*, const char*, const char*)"),
  start: lib.func("CavsJob* cavs_start_json(CavsContext*, const char*, const char*)"),
  poll: lib.func("CavsResult* cavs_job_poll(CavsJob*)"),
  cancel: lib.func("int cavs_job_cancel(CavsJob*)"),
  jobFree: lib.func("void cavs_job_free(CavsJob*)"),
  resultJson: lib.func("const char* cavs_result_json(CavsResult*)"),
  resultOk: lib.func("int cavs_result_ok(CavsResult*)"),
  resultFree: lib.func("void cavs_result_free(CavsResult*)"),
  stringFree: lib.func("void cavs_string_free(void*)"),
};

export type Pointer = unknown;
export type ProgressHandle = koffi.IKoffiRegisteredCallback;

export const native = {
  version(): string {
    return fns.version();
  },
  abiVersion(): string {
    return fns.abiVersion();
  },
  capabilitiesJson(): string {
    const ptr = fns.capabilities();
    if (!ptr) {
      return "";
    }
    const s = koffi.decode(ptr, "char", -1) as string;
    fns.stringFree(ptr);
    return s;
  },
  contextNew(): Pointer {
    return fns.ctxNew(null);
  },
  contextFree(ctx: Pointer): void {
    fns.ctxFree(ctx);
  },
  /** Register a progress callback pointer; returns it so it can stay alive
   *  and be unregistered afterwards. Pass null to clear. */
  registerProgress(fn: ((event: string) => void) | null): ProgressHandle | null {
    if (!fn) {
      return null;
    }
    return koffi.register((event: string) => fn(event), ProgressPtr);
  },
  setProgress(ctx: Pointer, cbPtr: ProgressHandle | null): void {
    fns.setProgress(ctx, cbPtr, null);
  },
  unregister(cbPtr: ProgressHandle | null): void {
    if (cbPtr) {
      koffi.unregister(cbPtr);
    }
  },
  executeSync(ctx: Pointer, op: string, req: string): string {
    const res = fns.execute(ctx, op, req);
    return drainResult(res);
  },
  startJob(ctx: Pointer, op: string, req: string): Pointer | null {
    return fns.start(ctx, op, req) ?? null;
  },
  /** Poll: the envelope JSON if finished, else null. */
  pollJob(job: Pointer): string | null {
    const res = fns.poll(job);
    if (!res) {
      return null;
    }
    return drainResult(res);
  },
  cancelJob(job: Pointer): void {
    fns.cancel(job);
  },
  freeJob(job: Pointer): void {
    fns.jobFree(job);
  },
};

function drainResult(res: Pointer): string {
  if (!res) {
    return '{"ok":false,"error":{"code":"CAVS-E-INTERNAL","message":"native returned null result","recoverable":false}}';
  }
  const json = fns.resultJson(res) as string;
  fns.resultFree(res);
  return json;
}
