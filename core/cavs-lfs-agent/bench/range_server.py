#!/usr/bin/env python3
"""Minimal static file server WITH HTTP Range support (python -m http.server
ignores Range, which would make per-chunk clients download whole packs).
Usage: range_server.py <directory>  — prints "PORT <n>" on stdout, logs one
line per request to stderr.

Set LATENCY_MS=<n> to sleep n ms before serving each request — a crude WAN
emulation (per-request latency dominates a chatty client on a real CDN)."""
import http.server
import os
import re
import sys
import time

LATENCY_S = int(os.environ.get("LATENCY_MS", "0")) / 1000.0


class Slice:
    def __init__(self, f, n):
        self.f, self.n = f, n

    def read(self, k=65536):
        if self.n <= 0:
            return b""
        b = self.f.read(min(k, self.n))
        self.n -= len(b)
        return b

    def close(self):
        self.f.close()


class Handler(http.server.SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def send_head(self):
        if LATENCY_S:
            time.sleep(LATENCY_S)
        path = self.translate_path(self.path)
        if os.path.isdir(path):
            return super().send_head()
        try:
            f = open(path, "rb")
        except OSError:
            self.send_error(404, "not found")
            return None
        size = os.fstat(f.fileno()).st_size
        rng = self.headers.get("Range")
        m = re.match(r"bytes=(\d+)-(\d*)$", rng or "")
        if m:
            start = int(m.group(1))
            end = min(int(m.group(2)) if m.group(2) else size - 1, size - 1)
            if start >= size:
                f.close()
                self.send_error(416, "range not satisfiable")
                return None
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("Content-Type", "application/octet-stream")
            self.end_headers()
            f.seek(start)
            return Slice(f, end - start + 1)
        self.send_response(200)
        self.send_header("Content-Length", str(size))
        self.send_header("Content-Type", "application/octet-stream")
        self.end_headers()
        return f


def main():
    os.chdir(sys.argv[1])
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"PORT {srv.server_address[1]}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
