# CAVS Native C ABI (`cavs_sdk.h`)

The CAVS SDKs bind to a small, stable C ABI over the Rust core, maintained
alongside the `cavs-ffi` crate and declared in
`core/cavs-ffi/include/cavs_sdk.h`. The surface is coarse on purpose — JSON
in, JSON out, plus opaque handles for long-running jobs. CAVS operations are
file-system and compression heavy, so JSON overhead at the boundary is
negligible, and the ABI stays stable as the Rust internals evolve.

This doc is for authors of new language bindings or embedders calling the
library directly. Application developers should use a language SDK
([Go](SDK_GO.md), [Kotlin](SDK_KOTLIN.md), [Node](SDK_NODE.md)) and the
shared [operations / envelope / error model](SDKS.md).

- **ABI version:** `1.0.0`
- **Header:** `core/cavs-ffi/include/cavs_sdk.h`

## Library names

| Platform | Library file |
|---|---|
| Linux / BSD | `libcavs_sdk.so` |
| macOS | `libcavs_sdk.dylib` |
| Windows | `cavs_sdk.dll` |

Supported targets: `linux-x86_64`, `linux-aarch64`, `macos-x86_64`,
`macos-aarch64`, `windows-x86_64`.

## Opaque handle types

```c
typedef struct CavsContext CavsContext;
typedef struct CavsResult  CavsResult;
typedef struct CavsJob     CavsJob;

typedef void (*CavsProgressCallback)(const char *event_json, void *user_data);
```

- `CavsContext` — an execution context. Cheap to create; holds an optional
  progress hook. Create one per logical client and reuse it across calls.
- `CavsResult` — the outcome of one operation: the full response envelope plus
  decoded `ok`/`error` fields.
- `CavsJob` — a handle to an operation running on a background thread.

## Version and capabilities

```c
const char *cavs_sdk_version(void);      /* static string; do NOT free */
const char *cavs_sdk_abi_version(void);  /* static string; do NOT free */
char       *cavs_sdk_capabilities_json(void); /* heap; free with cavs_string_free() */
```

- `cavs_sdk_version()` — SDK/engine semver (e.g. `1.2.0`). The pointer is
  `'static`; it must **not** be freed.
- `cavs_sdk_abi_version()` — the C ABI contract version (`1.0.0`). Also
  `'static`; do not free.
- `cavs_sdk_capabilities_json()` — the capability descriptor as a heap JSON
  string. **You own it**; release it with `cavs_string_free()`. Shape:

  ```json
  {
    "abiVersion": "1.0.0",
    "sdkVersion": "1.2.0",
    "schemaVersion": "1.0",
    "features": ["analyze", "packDirectory", "previewUpdate", "compareRoutes",
                 "createPlan", "applyPlan", "verifyInstall", "benchmark",
                 "estimateSavings"],
    "platform": { "os": "linux", "arch": "x86_64" }
  }
  ```

## Context

```c
CavsContext *cavs_context_new(const char *options_json);
void         cavs_context_free(CavsContext *ctx);

int cavs_context_set_progress_callback(CavsContext *ctx,
                                       CavsProgressCallback callback,
                                       void *user_data);
```

- `cavs_context_new(options_json)` — creates a context. `options_json` is
  currently **reserved**: pass `NULL` or `"{}"`. It exists so options can be
  added later without an ABI break. Returns a handle you must free with
  `cavs_context_free`.
- `cavs_context_free(ctx)` — frees a context. Tolerates `NULL`. Free exactly
  once.
- `cavs_context_set_progress_callback(ctx, callback, user_data)` — registers
  the progress callback and its opaque `user_data`. Pass a `NULL` `callback`
  to clear it. Returns `0` on success, `-1` if `ctx` is `NULL`. `user_data` is
  owned by the caller and only passed back verbatim to the callback.

## Result accessors

```c
const char *cavs_result_json(const CavsResult *result);
int         cavs_result_ok(const CavsResult *result);
const char *cavs_result_error_code(const CavsResult *result);
const char *cavs_result_error_message(const CavsResult *result);
void        cavs_result_free(CavsResult *result);
```

- `cavs_result_json(result)` — the full response envelope JSON (see
  [the envelope](SDKS.md#the-json-envelope)). The string is **owned by the
  result** and valid until `cavs_result_free`; do not free it separately.
  Returns `NULL` if `result` is `NULL`.
- `cavs_result_ok(result)` — `1` if the operation succeeded, `0` otherwise
  (including a `NULL` result).
- `cavs_result_error_code(result)` — the stable `CAVS-E-*` code, or `NULL`
  when the result is OK or `NULL`. Owned by the result.
- `cavs_result_error_message(result)` — the human-readable message, or `NULL`
  when OK / `NULL`. Owned by the result.
- `cavs_result_free(result)` — frees the result (and the strings it owns).
  Tolerates `NULL`. Free exactly once.

## Synchronous execution

```c
CavsResult *cavs_execute_json(CavsContext *ctx,
                             const char *operation,
                             const char *request_json);
```

Runs `operation` with `request_json` **on the calling thread** and returns a
`CavsResult`. `operation` is one of the eight operation names (see
[operations](SDKS.md#operations)); `request_json` is the request envelope
(or a bare `data` object). A `NULL` `request_json` is treated as an empty
object `{}` (some operations need no fields).

The result is effectively never `NULL` for readable inputs: engine errors are
encoded into the response envelope (with `ok:false` and an `error` object), so
there is a single success path. If `operation` is `NULL`/non-UTF-8 or
`request_json` is non-UTF-8, the returned result carries a
`CAVS-E-INVALID-REQUEST` / `CAVS-E-INVALID-JSON` envelope. Free the result
with `cavs_result_free`.

```c
CavsContext *ctx = cavs_context_new(NULL);
CavsResult *r = cavs_execute_json(
    ctx, "estimateSavings",
    "{\"schemaVersion\":\"1.0\",\"data\":{"
    "\"pricePerGb\":0.08,\"monthlyDownloads\":500000,"
    "\"averageFullDownloadBytes\":65011712,"
    "\"averageCavsDownloadBytes\":2631921}}");
if (cavs_result_ok(r)) {
    printf("%s\n", cavs_result_json(r));
} else {
    printf("error %s: %s\n",
           cavs_result_error_code(r), cavs_result_error_message(r));
}
cavs_result_free(r);
cavs_context_free(ctx);
```

## Asynchronous jobs

```c
CavsJob    *cavs_start_json(CavsContext *ctx,
                           const char *operation,
                           const char *request_json);
CavsResult *cavs_job_poll(CavsJob *job);   /* result if finished, else NULL */
int         cavs_job_cancel(CavsJob *job); /* 0 on success, -1 if NULL */
void        cavs_job_free(CavsJob *job);
```

Job lifecycle:

1. **Start.** `cavs_start_json` runs the operation on a **background thread**
   and returns a `CavsJob*`. It returns `NULL` only if `operation` /
   `request_json` are unreadable (as with the sync call, a `NULL`
   `request_json` becomes `{}`).
2. **Poll.** `cavs_job_poll(job)` returns the finished `CavsResult*`, or
   `NULL` while the job is still running. Poll on a short interval. After a
   non-`NULL` return the job is drained; free the returned result with
   `cavs_result_free`, and still free the job with `cavs_job_free`.
3. **Cancel (optional).** `cavs_job_cancel(job)` requests **cooperative**
   cancellation (sets a flag the operation checks between phases). Returns `0`
   on success, `-1` if `job` is `NULL`. A cancelled operation completes with a
   `CAVS-E-CANCELLED` envelope.
4. **Free.** `cavs_job_free(job)` frees the job. If it is still running,
   cancellation is requested and the worker thread is **joined first**, so no
   thread outlives the handle. Tolerates `NULL`. Always free every job exactly
   once.

```c
CavsJob *job = cavs_start_json(ctx, "createPlan", request_json);
CavsResult *r = NULL;
while ((r = cavs_job_poll(job)) == NULL) {
    /* sleep a short interval, do other work, or check cancellation */
}
/* use r ... */
cavs_result_free(r);
cavs_job_free(job);
```

## Progress callback and threading model

A registered callback is invoked as the operation emits events:

```c
typedef void (*CavsProgressCallback)(const char *event_json, void *user_data);
```

`event_json` is a JSON object (owned by the library, valid only for the
duration of the call — copy it if you need to keep it). `user_data` is the
opaque pointer you registered. Event shape (fields are omitted when absent):

```json
{ "type": "progress", "operation": "packDirectory", "phase": "chunking",
  "currentBytes": 1048576, "totalBytes": 8388608, "percentage": 0.125,
  "message": "assets/textures.pak" }
```

`type` is one of `started`, `phaseChanged`, `progress`, `completed`,
`failed`. Threading rules:

- `cavs_execute_json` runs on the **calling thread**, so the callback fires on
  the calling thread.
- `cavs_start_json` runs on a **background thread**, so the callback may be
  invoked from that thread. **Callbacks must be thread-safe.**

The callback and its `user_data` are stored on the context; they persist
across calls until you clear or replace them with
`cavs_context_set_progress_callback`.

## Memory ownership rules

| Returned by | Owner | How to release |
|---|---|---|
| `cavs_context_new` | caller | `cavs_context_free` (once) |
| `cavs_start_json` | caller | `cavs_job_free` (once) |
| `cavs_execute_json`, `cavs_job_poll` | caller | `cavs_result_free` (once) |
| `cavs_sdk_capabilities_json` | caller | `cavs_string_free` |
| `cavs_sdk_version`, `cavs_sdk_abi_version` | library (`'static`) | **do NOT free** |
| `cavs_result_json` / `_error_code` / `_error_message` | the `CavsResult` | freed by `cavs_result_free` — **do NOT free separately** |

```c
void cavs_string_free(char *ptr);
```

`cavs_string_free` frees a heap string returned by the library (i.e. the
capabilities JSON). It tolerates `NULL`. Do **not** use it on result-owned
strings or the static version strings.

Additional rules:

- Every `*_free` function is idempotent against `NULL` but must be called at
  most once per live handle.
- Strings you pass **in** (`operation`, `request_json`, `options_json`) are
  borrowed for the duration of the call only; the library does not retain
  them.
- A `NULL` `request_json` is treated as `{}`.

## ABI and schema versioning

Two independent version numbers govern the contract:

- **ABI version** (`cavs_sdk_abi_version()`, currently `1.0.0`) — the C
  contract: function signatures, handle semantics, ownership rules. The major
  is bumped only on a breaking change to the envelope or operation semantics.
- **Schema version** (the envelope's `schemaVersion`, currently `1.0`) — the
  JSON contract. Requests may set `schemaVersion`; only **major `1`** is
  accepted. Any other major is rejected with `CAVS-E-UNSUPPORTED-SCHEMA`.
  Omitting it is allowed (a bare `data` object is accepted).

Within a major version, new operations and new optional response fields may be
added without an ABI break — discover what a given library supports via
`cavs_sdk_capabilities_json()` (`features` array) rather than assuming.

## Error envelope

On failure the response envelope carries a stable code:

```json
{ "schemaVersion": "1.0", "ok": false, "operation": "analyze",
  "error": { "code": "CAVS-E-PATH-NOT-FOUND",
             "message": "/no/such/path does not exist",
             "recoverable": false, "details": {} } }
```

The complete `CAVS-E-*` code table (and which are `recoverable`) is in
[SDKS.md](SDKS.md#error-model). `cavs_result_error_code` /
`cavs_result_error_message` expose `code` and `message` directly for cheap
access without parsing the JSON.
