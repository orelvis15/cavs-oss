package cavs

import "fmt"

// Error is a CAVS operation failure carrying the stable engine error code.
type Error struct {
	Code        string         `json:"code"`
	Message     string         `json:"message"`
	Recoverable bool           `json:"recoverable"`
	Details     map[string]any `json:"details,omitempty"`
}

func (e *Error) Error() string {
	if e.Code == "" {
		return e.Message
	}
	return fmt.Sprintf("%s: %s", e.Code, e.Message)
}

// Common error codes, exported so callers can branch without string
// literals. The full set is defined by the engine.
const (
	CodePathNotFound     = "CAVS-E-PATH-NOT-FOUND"
	CodePathTraversal    = "CAVS-E-PATH-TRAVERSAL"
	CodeInvalidRequest   = "CAVS-E-INVALID-REQUEST"
	CodeUnknownOperation = "CAVS-E-UNKNOWN-OPERATION"
	CodeCancelled        = "CAVS-E-CANCELLED"
)

// IsCode reports whether err is a *Error with the given code.
func IsCode(err error, code string) bool {
	ce, ok := err.(*Error)
	return ok && ce.Code == code
}
