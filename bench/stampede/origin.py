"""The stampede check's origin (bench/stampede.sh): a slow origin that counts its fetches.

Every GET of a path under /stampede/ waits DELAY seconds, then answers 4 KiB fresh for 10 minutes
(Cache-Control: max-age=600), as fitbench's cache origin answers, and counts the request by its
path. GET /count/<path> answers how many times <path> was fetched; GET /ready answers 200. One
thread a connection, so that a cache that does not hold its clients back gets every one of them
answered, and the count says so.

    python3 -I origin.py PORT DELAY
"""

import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BODY = b"0123456789abcdef" * 256
counts = {}
lock = threading.Lock()
delay = 1.0


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def answer(self, status, body, headers=()):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/ready":
            self.answer(200, b"ok")
        elif path.startswith("/count/"):
            with lock:
                n = counts.get(path[len("/count"):], 0)
            self.answer(200, str(n).encode())
        elif path.startswith("/stampede/"):
            with lock:
                counts[path] = counts.get(path, 0) + 1
            time.sleep(delay)
            self.answer(200, BODY, [("Content-Type", "text/plain"), ("Cache-Control", "max-age=600")])
        else:
            self.answer(404, b"not found")


class Server(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 4096


if __name__ == "__main__":
    delay = float(sys.argv[2])
    Server(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
