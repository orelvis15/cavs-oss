package cavs

/*
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"
	"unsafe"
)

// cavsGoProgress is the C-callable trampoline the native library invokes
// for every progress event. user_data is the native context pointer, used
// to resolve the Go callback registered for that context. It may run on a
// native worker thread, so the registry lookup is mutex-guarded.
//
//export cavsGoProgress
func cavsGoProgress(eventJSON *C.char, userData unsafe.Pointer) {
	fn := lookupProgress(userData)
	if fn == nil {
		return
	}
	var event ProgressEvent
	if err := json.Unmarshal([]byte(C.GoString(eventJSON)), &event); err != nil {
		return
	}
	fn(event)
}
