"""End-to-end checks for verified fetches, traffic accounting, and bounded failures.

Build the release example before running these tests. RUGIX_HTTP_BENCHMARK_CLIENT
can point to another build of the driver.
"""

import concurrent.futures
import http.client
import os
import pathlib
import subprocess
import tempfile
import time
import unittest

from benchmark import ROOT, STRATEGIES, run_client
from server import BenchmarkServer

CLIENT = pathlib.Path(
    os.environ.get(
        "RUGIX_HTTP_BENCHMARK_CLIENT",
        ROOT / "target/release/examples/http-fetch-benchmark",
    )
).resolve()


class BenchmarkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="rugix-http-fetch-test-")
        cls.fixture = pathlib.Path(cls.temporary.name)
        subprocess.run(
            [str(CLIENT), "prepare", str(cls.fixture), "--payload-mib", "1"],
            check=True,
        )
        cls.bundle = (cls.fixture / "bundle.rugixb").read_bytes()

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_reproducible_fixture(self):
        """Independent fixture builds produce identical bundle bytes."""
        with tempfile.TemporaryDirectory(
            prefix="rugix-http-fixture-test-"
        ) as temporary:
            fixture = pathlib.Path(temporary)
            subprocess.run(
                [str(CLIENT), "prepare", str(fixture), "--payload-mib", "1"],
                check=True,
            )
            self.assertEqual((fixture / "bundle.rugixb").read_bytes(), self.bundle)

    def test_verified_decodes_and_accounting(self):
        """Every strategy reconstructs the payload with zero, partial, and full reuse."""
        for reuse, layout in (
            (0, "scattered"),
            (50, "scattered"),
            (50, "contiguous"),
            (100, "scattered"),
        ):
            for strategy in STRATEGIES:
                with self.subTest(reuse=reuse, layout=layout, strategy=strategy):
                    with BenchmarkServer(self.bundle) as server:
                        result = run_client(
                            CLIENT,
                            self.fixture,
                            server.url,
                            strategy,
                            reuse,
                            layout,
                            10,
                        )
                    self.assertEqual(result.payload_bytes, 1024 * 1024)
                    self.assertEqual(
                        result.reused_bytes, result.payload_bytes * reuse // 100
                    )
                    self.assertGreaterEqual(
                        result.source_read_bytes + result.source_skipped_bytes,
                        len(self.bundle),
                    )
                    self.assertGreaterEqual(
                        server.metrics.body_bytes, result.source_read_bytes
                    )
                    self.assertGreater(server.metrics.connections, 0)
                    if strategy == "dynamic":
                        self.assertGreater(server.metrics.range_requests, 0)
                        if reuse:
                            self.assertGreater(result.source_skipped_bytes, 0)
                    else:
                        self.assertGreaterEqual(
                            result.source_read_bytes, len(self.bundle)
                        )
                        self.assertEqual(result.source_skipped_bytes, 0)

    def test_production_pipeline_coalesces_and_overlaps_ranges(self):
        """Production workers overlap bounded ranges and return connections to the pool."""
        with tempfile.TemporaryDirectory(
            prefix="rugix-http-pipeline-test-"
        ) as temporary:
            fixture = pathlib.Path(temporary)
            subprocess.run(
                [str(CLIENT), "prepare", str(fixture), "--payload-mib", "6"],
                check=True,
            )
            bundle = (fixture / "bundle.rugixb").read_bytes()
            with BenchmarkServer(bundle, request_delay=0.02) as server:
                result = run_client(
                    CLIENT, fixture, server.url, "dynamic", 0, "scattered", 10
                )
        self.assertEqual(result.payload_bytes, 6 * 1024 * 1024)
        self.assertGreaterEqual(server.metrics.peak_in_flight, 2)
        self.assertLessEqual(server.metrics.peak_in_flight, 4)
        self.assertLessEqual(server.metrics.requests, 9)
        self.assertLessEqual(server.metrics.connections, 4)

    def test_configured_range_size_and_concurrency(self):
        """Tuning flags reach production HTTP acquisition and verified bundle decoding."""
        for concurrency in (1, 6):
            with self.subTest(concurrency=concurrency):
                with BenchmarkServer(self.bundle, request_delay=0.02) as server:
                    result = subprocess.run(
                        [
                            str(CLIENT), "fetch", str(self.fixture), server.url,
                            "--strategy", "dynamic",
                            "--http-concurrency", str(concurrency),
                            "--http-max-range-size", "32KiB",
                            "--http-stall-timeout", "2",
                        ],
                        check=True, capture_output=True, text=True, timeout=10,
                        env={**os.environ, "NO_PROXY": "*", "no_proxy": "*"},
                    )
                self.assertEqual(int(result.stdout.split(",")[1]), 1024 * 1024)
                self.assertGreaterEqual(server.metrics.range_requests, 32)
                self.assertLessEqual(server.metrics.peak_in_flight, concurrency)
                if concurrency == 1:
                    self.assertEqual(server.metrics.peak_in_flight, 1)
                else:
                    self.assertGreaterEqual(server.metrics.peak_in_flight, 2)

    def test_persistent_ranges(self):
        """Two range requests reuse one server connection and return exact byte slices."""
        with BenchmarkServer(self.bundle) as server:
            connection = http.client.HTTPConnection(
                "127.0.0.1", server.server_port, timeout=5
            )
            try:
                for start, end in ((0, 0), (10, 99)):
                    connection.request(
                        "GET",
                        "/bundle.rugixb",
                        headers={"Range": f"bytes={start}-{end}"},
                    )
                    response = connection.getresponse()
                    self.assertEqual(response.status, 206)
                    self.assertEqual(response.read(), self.bundle[start : end + 1])
            finally:
                connection.close()
        self.assertEqual(server.metrics.connections, 1)
        self.assertEqual(server.metrics.range_requests, 2)
        self.assertEqual(server.metrics.body_bytes, 91)

    def test_concurrent_requests_share_bandwidth(self):
        """Concurrent responses overlap while sharing the configured body-rate limit."""
        body_size = 64 * 1024
        rate = 512 * 1024
        with BenchmarkServer(self.bundle, request_delay=0.1, body_rate=rate) as server:

            def fetch_range():
                connection = http.client.HTTPConnection(
                    "127.0.0.1", server.server_port, timeout=5
                )
                try:
                    connection.request(
                        "GET",
                        "/bundle.rugixb",
                        headers={"Range": f"bytes=0-{body_size - 1}"},
                    )
                    return connection.getresponse().read()
                finally:
                    connection.close()

            started = time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(lambda _: fetch_range(), range(2)))
            elapsed = time.monotonic() - started
        self.assertEqual(results, [self.bundle[:body_size]] * 2)
        self.assertEqual(server.metrics.body_bytes, body_size * 2)
        self.assertEqual(server.metrics.peak_in_flight, 2)
        self.assertGreaterEqual(elapsed, body_size * 2 / rate)

    def test_corrupt_bundle_is_rejected(self):
        """A failed production hash check fails the sample instead of reporting throughput."""
        corrupt = bytearray(self.bundle)
        corrupt[len(corrupt) // 2] ^= 1
        with (
            BenchmarkServer(corrupt) as server,
            self.assertRaises(subprocess.CalledProcessError) as raised,
        ):
            run_client(CLIENT, self.fixture, server.url, "dynamic", 0, "scattered", 10)
        self.assertIn("invalid block hash", raised.exception.stderr)

    def test_stalled_sample_is_bounded(self):
        """The harness terminates a stalled client before its transport timeout fires."""
        with (
            BenchmarkServer(self.bundle, request_delay=5) as server,
            self.assertRaises(subprocess.TimeoutExpired),
        ):
            run_client(CLIENT, self.fixture, server.url, "dynamic", 0, "scattered", 0.1)


if __name__ == "__main__":
    unittest.main()
