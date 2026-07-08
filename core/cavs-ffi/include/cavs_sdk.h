/*
 * cavs_sdk.h — stable C ABI for the CAVS SDK native library.
 *
 * Generated/maintained alongside `core/cavs-ffi`. ABI version: 1.0.0.
 *
 * Library names by platform:
 *   Linux/*BSD : libcavs_sdk.so
 *   macOS      : libcavs_sdk.dylib
 *   Windows    : cavs_sdk.dll
 *
 * Memory ownership:
 *   - Handles (CavsContext*, CavsResult*, CavsJob*) are freed by their
 *     matching *_free function, exactly once.
 *   - char* returned by cavs_sdk_capabilities_json() must be freed with
 *     cavs_string_free().
 *   - char* returned by cavs_sdk_version()/cavs_sdk_abi_version() is static
 *     and must NOT be freed.
 *   - char* returned by cavs_result_json()/error_code()/error_message() is
 *     owned by the CavsResult and freed by cavs_result_free().
 *
 * Threading:
 *   - cavs_execute_json() runs on the calling thread.
 *   - cavs_start_json() runs on a background thread; a registered progress
 *     callback may be invoked from that thread and must be thread-safe.
 */
#ifndef CAVS_SDK_H
#define CAVS_SDK_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct CavsContext CavsContext;
typedef struct CavsResult CavsResult;
typedef struct CavsJob CavsJob;

typedef void (*CavsProgressCallback)(const char *event_json, void *user_data);

/* --- Version / capabilities --------------------------------------------- */

/* Static strings; do NOT free. */
const char *cavs_sdk_version(void);
const char *cavs_sdk_abi_version(void);

/* Heap string; free with cavs_string_free(). */
char *cavs_sdk_capabilities_json(void);

/* --- Context ------------------------------------------------------------- */

CavsContext *cavs_context_new(const char *options_json);
void cavs_context_free(CavsContext *ctx);

/* Returns 0 on success, -1 if ctx is NULL. Pass a NULL callback to clear. */
int cavs_context_set_progress_callback(CavsContext *ctx,
                                       CavsProgressCallback callback,
                                       void *user_data);

/* --- Synchronous execution ---------------------------------------------- */

/* Runs on the calling thread. Free the result with cavs_result_free(). */
CavsResult *cavs_execute_json(CavsContext *ctx,
                             const char *operation,
                             const char *request_json);

/* --- Asynchronous jobs --------------------------------------------------- */

CavsJob *cavs_start_json(CavsContext *ctx,
                        const char *operation,
                        const char *request_json);

/* Returns the result if finished, else NULL (still running). */
CavsResult *cavs_job_poll(CavsJob *job);

/* Request cooperative cancellation. Returns 0 on success, -1 if NULL. */
int cavs_job_cancel(CavsJob *job);

/* Frees the job; joins the worker thread first. */
void cavs_job_free(CavsJob *job);

/* --- Result accessors ---------------------------------------------------- */

const char *cavs_result_json(const CavsResult *result);
int cavs_result_ok(const CavsResult *result);
const char *cavs_result_error_code(const CavsResult *result);
const char *cavs_result_error_message(const CavsResult *result);
void cavs_result_free(CavsResult *result);

/* --- Misc ---------------------------------------------------------------- */

void cavs_string_free(char *ptr);

#ifdef __cplusplus
}
#endif

#endif /* CAVS_SDK_H */
