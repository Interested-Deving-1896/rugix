#!/usr/bin/env python3
"""Run reproducible block-fetch workloads against the production Rugix HTTP source."""

import argparse
import csv
import dataclasses
import datetime
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys
import tempfile

from server import BenchmarkServer

ROOT = pathlib.Path(__file__).resolve().parents[2]
STRATEGIES = ("disabled", "resume", "dynamic")


@dataclasses.dataclass(frozen=True)
class ClientResult:
    elapsed_seconds: float
    payload_bytes: int
    reused_bytes: int
    source_read_bytes: int
    source_skipped_bytes: int


def run_client(client, fixture, url, strategy, reuse_percent, layout, timeout):
    """Run one verified decode, bounding stalls independently of fetcher timeouts."""
    environment = os.environ.copy()
    environment["NO_PROXY"] = "*"
    environment["no_proxy"] = "*"
    completed = subprocess.run(
        [
            str(client),
            "fetch",
            str(fixture),
            url,
            "--strategy",
            strategy,
            "--reuse-percent",
            str(reuse_percent),
            "--layout",
            layout,
        ],
        env=environment,
        check=True,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    elapsed, payload, reused, read, skipped = completed.stdout.strip().split(",")
    return ClientResult(
        float(elapsed), int(payload), int(reused), int(read), int(skipped)
    )


def positive_int(value):
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return parsed


def nonnegative_float(value):
    parsed = float(value)
    if not 0 <= parsed < float("inf"):
        raise argparse.ArgumentTypeError("must be finite and nonnegative")
    return parsed


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--client", type=pathlib.Path, help="Use an already-built driver"
    )
    parser.add_argument(
        "--output", type=pathlib.Path, help="Write CSV here (default: stdout)"
    )
    parser.add_argument("--payload-mib", type=positive_int, default=8)
    parser.add_argument("--block-kib", type=positive_int, default=64)
    parser.add_argument("--repetitions", type=positive_int, default=3)
    parser.add_argument("--request-delay-ms", type=nonnegative_float, default=65)
    parser.add_argument("--connection-delay-ms", type=nonnegative_float, default=0)
    parser.add_argument(
        "--bandwidth-mib-s", type=nonnegative_float, default=8, help="0: unlimited"
    )
    parser.add_argument("--timeout-seconds", type=positive_int, default=300)
    parser.add_argument(
        "--reuse-percent", type=int, choices=range(101), default=83, metavar="0..100"
    )
    parser.add_argument(
        "--strategies", nargs="+", choices=STRATEGIES, default=STRATEGIES
    )
    args = parser.parse_args()
    if args.block_kib > 65535 or args.payload_mib * 1024 % args.block_kib:
        parser.error("block size must fit u16 and divide the payload size")
    return args


def benchmark(args, output, revision, dirty):
    client = args.client
    if client is None:
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--release",
                "-p",
                "rugix-ctrl",
                "--example",
                "http-fetch-benchmark",
            ],
            cwd=ROOT,
            check=True,
        )
        metadata = subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            text=True,
        )
        client = (
            pathlib.Path(json.loads(metadata)["target_directory"])
            / "release/examples/http-fetch-benchmark"
        )
    client = client.resolve()
    client_hash = hashlib.sha256(client.read_bytes()).hexdigest()
    timestamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
    with tempfile.TemporaryDirectory(prefix="rugix-http-fetch-") as temporary:
        fixture = pathlib.Path(temporary)
        subprocess.run(
            [
                str(client),
                "prepare",
                str(fixture),
                "--payload-mib",
                str(args.payload_mib),
                "--block-kib",
                str(args.block_kib),
            ],
            check=True,
        )
        bundle = (fixture / "bundle.rugixb").read_bytes()
        bundle_hash = hashlib.sha256(bundle).hexdigest()
        cases = (
            ("no-reuse", 0, "scattered"),
            ("scattered-reuse", args.reuse_percent, "scattered"),
            ("contiguous-reuse", args.reuse_percent, "contiguous"),
        )
        writer = None
        for repetition in range(1, args.repetitions + 1):
            strategies = list(args.strategies)
            rotation = (repetition - 1) % len(strategies)
            strategies = strategies[rotation:] + strategies[:rotation]
            for scenario, reuse_percent, layout in cases:
                for strategy in strategies:
                    with BenchmarkServer(
                        bundle,
                        connection_delay=args.connection_delay_ms / 1000,
                        request_delay=args.request_delay_ms / 1000,
                        body_rate=args.bandwidth_mib_s * 1024 * 1024,
                    ) as server:
                        result = run_client(
                            client,
                            fixture,
                            server.url,
                            strategy,
                            reuse_percent,
                            layout,
                            args.timeout_seconds,
                        )
                    row = {
                        "revision": revision,
                        "dirty": dirty,
                        "timestamp_utc": timestamp,
                        "host": platform.platform(),
                        "client_sha256": client_hash,
                        "bundle_sha256": bundle_hash,
                        "bundle_bytes": len(bundle),
                        "block_kib": args.block_kib,
                        "request_delay_ms": args.request_delay_ms,
                        "connection_delay_ms": args.connection_delay_ms,
                        "bandwidth_mib_s": args.bandwidth_mib_s,
                        "scenario": scenario,
                        "strategy": strategy,
                        "repetition": repetition,
                        "requested_reuse_percent": reuse_percent,
                        **dataclasses.asdict(result),
                        **dataclasses.asdict(server.metrics),
                        "payload_mib_s": result.payload_bytes
                        / result.elapsed_seconds
                        / 1024
                        / 1024,
                        "source_mib_s": result.source_read_bytes
                        / result.elapsed_seconds
                        / 1024
                        / 1024,
                    }
                    if writer is None:
                        writer = csv.DictWriter(output, fieldnames=row.keys())
                        writer.writeheader()
                    writer.writerow(row)
                    output.flush()
                    print(
                        f"{scenario:18} {strategy:8} #{repetition}: "
                        f"{result.elapsed_seconds:.3f}s, {server.metrics.requests} requests, "
                        f"{server.metrics.connections} connections, "
                        f"{result.source_read_bytes / 1024 / 1024:.2f} MiB read",
                        file=sys.stderr,
                    )


def main():
    args = parse_args()
    try:
        revision = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
        dirty = bool(
            subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)
        )
        if args.output:
            with args.output.open("w", newline="") as output:
                benchmark(args, output, revision, dirty)
        else:
            benchmark(args, sys.stdout, revision, dirty)
    except subprocess.CalledProcessError as error:
        print(error.stderr or str(error), file=sys.stderr)
        return 1
    except subprocess.TimeoutExpired:
        print(
            f"client exceeded {args.timeout_seconds}s; benchmark aborted",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
