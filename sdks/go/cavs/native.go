// Package cavs is the Go SDK for CAVS. It loads the same compiled Rust
// core the CAVS CLI uses (through a stable C ABI) and exposes idiomatic Go
// APIs for analyzing builds, previewing update cost, packing artifacts,
// creating/applying update plans, verifying installs, benchmarking
// delivery routes and estimating bandwidth savings.
//
// The native library (libcavs_sdk.{so,dylib} / cavs_sdk.dll) and its
// header are expected under cavs/native/ — run `make native` at the SDK
// root to build and stage them from the Rust workspace.
package cavs

/*
#cgo CFLAGS: -I${SRCDIR}/native/include
#cgo darwin LDFLAGS: -L${SRCDIR}/native -lcavs_sdk -Wl,-rpath,${SRCDIR}/native
#cgo linux LDFLAGS: -L${SRCDIR}/native -lcavs_sdk -Wl,-rpath,${SRCDIR}/native -ldl -lm
#include <stdlib.h>
#include "cavs_sdk.h"

// Trampoline defined in native_cb.go via //export.
extern void cavsGoProgress(const char *event_json, void *user_data);
*/
import "C"

import (
	"sync"
	"unsafe"
)

// The native library carries the progress callback as an opaque void*
// user_data. Passing a Go closure pointer through C is awkward and trips
// `go vet`'s unsafe-pointer checks, so instead we pass the (real) native
// context pointer as user_data and resolve the Go callback from this
// registry keyed by that pointer.
var (
	progressMu       sync.RWMutex
	progressRegistry = map[unsafe.Pointer]func(ProgressEvent){}
)

// nativeContext wraps a *CavsContext.
type nativeContext struct {
	ptr *C.CavsContext
}

func newNativeContext() *nativeContext {
	return &nativeContext{ptr: C.cavs_context_new(nil)}
}

func (n *nativeContext) free() {
	if n == nil || n.ptr == nil {
		return
	}
	progressMu.Lock()
	delete(progressRegistry, unsafe.Pointer(n.ptr))
	progressMu.Unlock()
	C.cavs_context_free(n.ptr)
	n.ptr = nil
}

// setProgress registers (fn != nil) or clears (fn == nil) the progress
// callback for this context.
func (n *nativeContext) setProgress(fn func(ProgressEvent)) {
	key := unsafe.Pointer(n.ptr)
	progressMu.Lock()
	if fn == nil {
		delete(progressRegistry, key)
	} else {
		progressRegistry[key] = fn
	}
	progressMu.Unlock()

	if fn == nil {
		C.cavs_context_set_progress_callback(n.ptr, nil, nil)
		return
	}
	C.cavs_context_set_progress_callback(
		n.ptr,
		C.CavsProgressCallback(C.cavsGoProgress),
		key,
	)
}

func lookupProgress(userData unsafe.Pointer) func(ProgressEvent) {
	progressMu.RLock()
	defer progressMu.RUnlock()
	return progressRegistry[userData]
}

// job is a running native operation.
type job struct {
	ptr *C.CavsJob
}

func (n *nativeContext) start(operation, requestJSON string) *job {
	cOp := C.CString(operation)
	cReq := C.CString(requestJSON)
	defer C.free(unsafe.Pointer(cOp))
	defer C.free(unsafe.Pointer(cReq))
	ptr := C.cavs_start_json(n.ptr, cOp, cReq)
	if ptr == nil {
		return nil
	}
	return &job{ptr: ptr}
}

// poll returns the response envelope JSON and true once finished, or
// ("", false) while still running.
func (j *job) poll() (string, bool) {
	res := C.cavs_job_poll(j.ptr)
	if res == nil {
		return "", false
	}
	defer C.cavs_result_free(res)
	return C.GoString(C.cavs_result_json(res)), true
}

func (j *job) cancel() { C.cavs_job_cancel(j.ptr) }

func (j *job) free() {
	if j.ptr != nil {
		C.cavs_job_free(j.ptr)
		j.ptr = nil
	}
}

// Version returns the native SDK semver.
func Version() string { return C.GoString(C.cavs_sdk_version()) }

// ABIVersion returns the native C ABI contract version.
func ABIVersion() string { return C.GoString(C.cavs_sdk_abi_version()) }

// CapabilitiesJSON returns the native capability descriptor as JSON.
func CapabilitiesJSON() string {
	ptr := C.cavs_sdk_capabilities_json()
	if ptr == nil {
		return ""
	}
	defer C.cavs_string_free(ptr)
	return C.GoString(ptr)
}
