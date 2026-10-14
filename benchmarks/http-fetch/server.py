"""Loopback range server with controlled delays and an aggregate body-rate limit."""

import dataclasses
import http.server
import re
import socket
import threading
import time


@dataclasses.dataclass
class Metrics:
    connections: int = 0
    requests: int = 0
    range_requests: int = 0
    peak_in_flight: int = 0
    body_bytes: int = 0


class BenchmarkServer(http.server.ThreadingHTTPServer):
    """Serve one in-memory bundle and count traffic independently of the client."""

    daemon_threads = False

    def __init__(self, bundle, connection_delay=0.0, request_delay=0.0, body_rate=0.0):
        super().__init__(("127.0.0.1", 0), BundleHandler)
        self.bundle = bundle
        self.connection_delay = connection_delay
        self.request_delay = request_delay
        self.body_rate = body_rate
        self.metrics = Metrics()
        self.lock = threading.Lock()
        self.active_requests = 0
        self.next_body_time = 0.0
        self.stopping = threading.Event()
        self.thread = threading.Thread(
            target=self.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
        )

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}/bundle.rugixb"

    def get_request(self):
        connection, address = super().get_request()
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.settimeout(5)
        with self.lock:
            self.metrics.connections += 1
        return connection, address

    def pace_body(self, size):
        """Reserve bandwidth across all connections so concurrency shares one limit."""
        if self.body_rate == 0:
            return self.stopping.is_set()
        with self.lock:
            now = time.monotonic()
            self.next_body_time = max(now, self.next_body_time) + size / self.body_rate
            delay = self.next_body_time - now
        return self.stopping.wait(delay)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stopping.set()
        self.shutdown()
        self.server_close()
        self.thread.join()


class BundleHandler(http.server.BaseHTTPRequestHandler):
    """Support persistent HTTP/1.1 connections and single byte ranges."""

    protocol_version = "HTTP/1.1"

    def setup(self):
        self.server.stopping.wait(self.server.connection_delay)
        super().setup()

    def handle(self):
        """A source may close a connection with unread response bytes after a skip."""
        try:
            super().handle()
        except (ConnectionError, TimeoutError):
            self.close_connection = True

    def do_GET(self):
        server = self.server
        with server.lock:
            server.metrics.requests += 1
            server.active_requests += 1
            server.metrics.peak_in_flight = max(
                server.metrics.peak_in_flight, server.active_requests
            )
        try:
            self.respond()
        finally:
            with server.lock:
                server.active_requests -= 1

    def respond(self):
        server = self.server
        if server.stopping.wait(server.request_delay):
            self.close_connection = True
            return
        if self.path != "/bundle.rugixb":
            self.send_error(404)
            return
        size = len(server.bundle)
        start, end = 0, size - 1
        range_header = self.headers.get("Range")
        if range_header is not None:
            match = re.fullmatch(r"bytes=(\d+)-(\d*)", range_header)
            if match is None:
                self.send_error(400, "Expected a single byte range")
                return
            start = int(match[1])
            end = min(int(match[2]) if match[2] else end, end)
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{size}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            with server.lock:
                server.metrics.range_requests += 1
        self.send_response(206 if range_header is not None else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        if range_header is not None:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        try:
            for offset in range(start, end + 1, 16 * 1024):
                chunk = server.bundle[offset : min(offset + 16 * 1024, end + 1)]
                if server.pace_body(len(chunk)):
                    self.close_connection = True
                    return
                self.wfile.write(chunk)
                with server.lock:
                    server.metrics.body_bytes += len(chunk)
        except (ConnectionError, TimeoutError):
            self.close_connection = True

    def log_message(self, *_):
        pass
