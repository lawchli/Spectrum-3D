#!/usr/bin/env python3
"""Serve the web app on localhost. The stock http.server drops requests when a browser loads
many modules at once on a cold start; this one queues them."""

from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class WebServer(ThreadingHTTPServer):
    request_queue_size = 128
    daemon_threads = True


if __name__ == "__main__":
    web = Path(__file__).resolve().parent.parent / "web"
    handler = partial(SimpleHTTPRequestHandler, directory=str(web))
    with WebServer(("127.0.0.1", 8765), handler) as server:
        server.serve_forever()
