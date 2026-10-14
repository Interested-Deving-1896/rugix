# HTTP Fetch Benchmark

This benchmark reproduces the small sequential range requests reported in
[issue #136](https://github.com/rugix/rugix/issues/136). It measures the production
HTTP source together with the block-based bundle decoder so that connection reuse,
range coalescing, and concurrent fetching can be compared against the same workloads.

## Run the Benchmark

Run from the repository root. Prerequisites are Python 3.11 or later and the
repository's Rust toolchain and native build dependencies. Python uses only its
standard library. No device, root privileges, network shaping, or external service
is required.

```sh
python3 benchmarks/http-fetch/benchmark.py --output benchmarks/http-fetch/baseline.csv
```

The script builds a release example with the locked dependencies, generates a
temporary bundle, runs three repetitions per scenario and strategy, and writes
one CSV row per verified sample. Progress goes to stderr. The default workload is
8 MiB, with 64 KiB blocks, a 65 ms delay per request, and an aggregate response-body
rate limit of 8 MiB/s. The example uses the repository's release profile.

To isolate connection setup overhead, add a separate delay per accepted connection:

```sh
python3 benchmarks/http-fetch/benchmark.py \
  --connection-delay-ms 130 --output benchmarks/http-fetch/connection-delay.csv
```

To measure larger bundles or smaller blocks:

```sh
python3 benchmarks/http-fetch/benchmark.py \
  --payload-mib 64 --block-kib 16 --repetitions 5 \
  --timeout-seconds 1800 --output benchmarks/http-fetch/larger-bundle.csv
```

Set either delay to zero to remove it. Set `--bandwidth-mib-s 0` for unlimited
body throughput. `--reuse-percent` changes the partial-reuse scenarios. Block size
must divide payload size. A client failure or a sample exceeding
`--timeout-seconds` aborts the run with a nonzero exit status; preceding CSV rows
remain available.

To reuse a build, pass `--client target/release/examples/http-fetch-benchmark`.
Rebuild after changing Rust code. An alternate client must be built from the
revision being measured. The recorded revision describes the working repository;
`client_sha256` identifies the executable actually measured.

## Workloads and Measurement

A fixture is a real Rugix bundle containing unique, deterministic, uncompressed
blocks with SHA-512/256 hashes and no deduplication. Fixed blocks make reuse counts
and layouts exact; this fixture does not model compressed Casync blocks or reuse
patterns from real filesystem images. Locally reusable blocks are
selected from a copy of its payload and exposed through `StoredBlockProvider`.
The scenarios are:

| Scenario | Locally Reusable Blocks |
| --- | --- |
| `no-reuse` | None, while still requesting dynamic range mode. |
| `scattered-reuse` | 83% by default, distributed evenly through the payload. |
| `contiguous-reuse` | The same reusable count at the start, leaving a consecutive run of missing blocks. |

The reused count is rounded down to whole blocks and recorded as `reused_bytes`.
Each scenario runs with `disabled` (one full response), `resume` (full streaming
after a range-support probe), and `dynamic` (proactive range requests).
Use `--strategies disabled dynamic` to limit the comparison. Strategy order rotates
between repetitions.

The timer starts before `HttpSource::new` and ends after decoding and validating
the entire bundle. It includes range probing, metadata, HTTP transfers, local
reusable-block reads, and production block and payload hash verification. Fixture
generation, provider indexing, process startup, and server shutdown are excluded.
Decoded output is discarded, so partition writes, hooks, boot configuration, and
reboots are excluded.

The driver compiles `crates/apps/rugix-ctrl/src/http_source/mod.rs` directly and
uses the production bundle decoder and range-worker pipeline. It supplies only a
system-error alias to avoid compiling the installer into this example. HTTP
retries are disabled so failing samples cannot silently include backoff; the
block fetcher can still request replacements for rejected downloaded blocks.

A fresh HTTP/1.1 server binds to an ephemeral loopback port for every sample. It
supports keep-alive and byte ranges, serves the bundle from memory, and applies:

- A connection delay once per accepted TCP connection.
- A request delay before each response's headers.
- A shared body-rate limit across all connections, including concurrent ones.

These are application delays, not simulated TCP round-trip times. Traffic uses
HTTP; TLS handshakes, DNS lookup, packet loss, TCP congestion control, and remote
server behavior are excluded. Connection delay provides a controlled way to expose
the cost of opening connections. Measurements on a real device and HTTPS endpoint
are still needed to establish deployment throughput.

## Interpret Results

CSV rows include the Git revision, working-tree dirty flag, host platform, client
and fixture SHA-256 hashes, workload settings, elapsed seconds, payload size,
reused bytes, source bytes read and skipped, accepted connections, HTTP requests,
range requests, peak concurrent requests, and response-body bytes written by the
server. Repository metadata is captured before creating the output file.

`payload_mib_s` is reconstructed payload size divided by elapsed time.
`source_mib_s` uses bytes consumed by the HTTP source. Compare elapsed time and
request counts together: coalescing can improve throughput while downloading
some otherwise reusable bytes.

The server's `body_bytes` includes the probe and bytes written into socket buffers
that a client may discard. It excludes HTTP headers and TCP overhead. The current
source's read counter records actual response-body bytes, including overlapping
metadata reads, probes, and retries. Its skipped counter is the bundle size minus
the union of consumed intervals, measured after all readers close. Their sum can
exceed bundle size. The original baseline counters described sequential source
traversal, so small metadata/probe differences should not be interpreted as payload
savings. A sample succeeds only after the production decoder validates its block
and payload hashes and expected payload size.

Compare medians across repetitions using identical workload settings, fixture
hashes, build profiles, and hardware. Keep both CSV files when comparing revisions:

```sh
python3 benchmarks/http-fetch/benchmark.py --output benchmarks/http-fetch/before.csv
# Apply the fetcher change, then rebuild and run the same command.
python3 benchmarks/http-fetch/benchmark.py --output benchmarks/http-fetch/after.csv
```

## Initial Baseline

The initial implementation was measured on 2026-10-13 in an x86_64 Linux workspace
using the default workload and three repetitions. Production source revision:
`b58bb3e`. These are medians, with request and connection counts per sample:

| Scenario | Strategy | Seconds | Requests | Connections | Source Read (MiB) |
| --- | --- | ---: | ---: | ---: | ---: |
| No reuse | Disabled | 1.131 | 1 | 1 | 8.004 |
| No reuse | Resume | 1.201 | 2 | 2 | 8.004 |
| No reuse | Dynamic | 9.728 | 130 | 130 | 8.004 |
| Scattered reuse | Disabled | 1.136 | 1 | 1 | 8.004 |
| Scattered reuse | Resume | 1.202 | 2 | 2 | 8.004 |
| Scattered reuse | Dynamic | 1.887 | 25 | 25 | 1.438 |
| Contiguous reuse | Disabled | 1.125 | 1 | 1 | 8.004 |
| Contiguous reuse | Resume | 1.195 | 2 | 2 | 8.004 |
| Contiguous reuse | Dynamic | 1.868 | 25 | 25 | 1.438 |

Dynamic mode opened one connection per request. It also made the same number of
requests for scattered and consecutive missing blocks. These counts provide a
baseline for connection reuse and range coalescing; elapsed times depend on the
machine and synthetic network settings.

## Pipeline Comparison

The threaded pipeline was measured with the same fixture and settings, on the
same machine on 2026-10-13. These medians come from three repetitions of the
uncommitted implementation based on `b58bb3e`:

| Scenario | Strategy | Seconds | Requests | Connections | Source Read (MiB) |
| --- | --- | ---: | ---: | ---: | ---: |
| No reuse | Disabled | 1.147 | 1 | 1 | 8.004 |
| No reuse | Resume | 1.202 | 2 | 1 | 8.004 |
| No reuse | Dynamic | 1.336 | 11 | 4 | 8.008 |
| Scattered reuse | Disabled | 1.129 | 1 | 1 | 8.004 |
| Scattered reuse | Resume | 1.199 | 2 | 1 | 8.004 |
| Scattered reuse | Dynamic | 0.689 | 25 | 4 | 1.383 |
| Contiguous reuse | Disabled | 1.131 | 1 | 1 | 8.004 |
| Contiguous reuse | Resume | 1.197 | 2 | 1 | 8.004 |
| Contiguous reuse | Dynamic | 0.445 | 5 | 2 | 1.383 |

Dynamic mode now coalesces consecutive missing bytes into requests of at most
1 MiB, keeps up to four requests outstanding, and reuses pooled connections.
Scattered missing blocks still require separate ranges, but their requests can
overlap. Dynamic mode retains probe and metadata overhead when nothing is reused;
a full streaming response remains slightly faster for that synthetic case.

The driver accepts the same `--http-concurrency`, `--http-max-range-size`, and
`--http-stall-timeout` options as Ctrl. The Python comparison runner uses their
defaults. To inspect a tuned workload, use an existing fixture directory:

```sh
target/release/examples/http-fetch-benchmark fetch FIXTURE URL --strategy dynamic \
  --http-concurrency 2 --http-max-range-size 256KiB --http-stall-timeout 120
```

The [reader module documentation](../../crates/libs/rugix-bundle/src/reader/mod.rs)
explains the source abstraction, fallback verification, buffering bounds, and
shutdown behavior.

## Check the Harness

```sh
cargo build --locked --release -p rugix-ctrl --example http-fetch-benchmark
python3 -m unittest discover -s benchmarks/http-fetch -v
cargo clippy --locked -p rugix-ctrl --example http-fetch-benchmark -- -D warnings
cargo fmt --check
```

The checks exercise reproducible fixture creation, actual bundle decoding with zero, partial, and full reuse,
traffic accounting, persistent range responses, shared bandwidth for concurrent
requests, rejection of corrupt blocks, and termination of a stalled client.
