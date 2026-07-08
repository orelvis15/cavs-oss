# Build the native CAVS library from the Rust workspace and stage it (plus
# the C header) where the cgo package expects it. Run `make native` before
# `go test ./...` or `go build ./...`.

REPO_ROOT := $(shell cd ../.. && pwd)
NATIVE_DIR := cavs/native
UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)
	LIB := libcavs_sdk.dylib
else
	LIB := libcavs_sdk.so
endif

.PHONY: native test build clean

native:
	cargo build --release -p cavs-ffi --manifest-path $(REPO_ROOT)/Cargo.toml
	mkdir -p $(NATIVE_DIR)/include
	cp $(REPO_ROOT)/core/cavs-ffi/include/cavs_sdk.h $(NATIVE_DIR)/include/
	cp $(REPO_ROOT)/target/release/$(LIB) $(NATIVE_DIR)/

test: native
	go test ./...

build: native
	go build ./...

clean:
	rm -f $(NATIVE_DIR)/*.so $(NATIVE_DIR)/*.dylib $(NATIVE_DIR)/*.dll
