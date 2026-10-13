# Bundle Fetching

Bundle installation overlaps HTTP range downloads with block verification,
decompression, and writes. It uses blocking threads and the existing bundle format.

## Source Capabilities

`BundleSource::RangeSource` owns an `Arc<dyn RangeSource>`;
`BundleSource::StreamSource` owns a `Box<dyn Read + Send>` stream. A range source
has a fixed length and opens owned readers for half-open byte intervals from
`&self`. Each reader keeps its own position.

The public `rugix_bundle::reader` module contains the source, reader, target,
progress, and local-block-provider types. Its module documentation explains the
complete reader pipeline. `BundleReader::start(source, expected_hash)` takes
ownership of a source and parses its header. Callers inspect the header, verify
signatures as needed, and consume payloads through `next_payload()` and
`decode_into()`. Both source variants use this same interface.

The reader privately tracks the parser position and buffers framing reads.
Sequential reading and skipping belong to `format::stlv::Read`, which accepts
ordinary Rust readers and a private bundle cursor. STLV skipping advances a range
cursor without downloading the skipped interval, while a stream drains its bytes.
The block fetcher obtains independent range access internally. After a ranged
payload is consumed, the parser resumes at its encoded end.

Progress callbacks receive a `BundleProgress` snapshot with the logical bundle
position and optional total length. They do not expose the parser input.
`BundleReader::with_size_hint()` supplies a transport's known length for a stream.
HTTP traffic statistics remain available through the separate download tracker;
readers must close before the tracker reports complete interval coverage.

`BundleSource::from_seekable` adapts files and other seekable inputs. It
serializes seek/read pairs under a mutex while preserving each range reader's
private position. Metadata-only STLV operations can use ordinary buffered readers
directly. `format::stlv::SeekReader` preserves efficient skipping of large values
for tools such as structure printing and signature rewriting.

## Planning and Acquisition

After validating the payload header, `BlockFetcher` builds a plan of encoded
intervals, local candidates, and references to earlier decoded blocks. It checks
that physical block sizes exactly cover the encoded payload before starting
workers. Encoded offsets remain fixed even when compression makes decoded sizes
unknown until installation.

Consecutive missing intervals are coalesced into requests of at most 1 MiB by
default. `RangeFetcher` keeps up to four requests outstanding by default.
`BundleReader::start_with_options` accepts public `ReaderOptions` to change the
maximum range size and concurrency; the size bound also applies to metadata and
fallback requests. Each request has a private
result slot, so a later response cannot prevent an earlier response from
completing. The installer consumes blocks in payload order while workers fetch
later bytes.

A stored-block index supplies candidates. The fetcher reads and hashes a candidate
before using it or skipping its encoded bytes. An unreadable or mismatching
candidate triggers an urgent request for that block's original interval. Urgent
jobs run before queued lookahead jobs and have a separate result slot. Downloaded
blocks are decompressed and hashed before being returned to the writer. A rejected
download can request two replacements; persistent corruption fails installation.

Deduplicated blocks are read from previously written target offsets and verified.
The writer, payload hasher, target reads, and decoded block index all remain on
the installation thread.

A forward stream is decoded sequentially. Local candidates are verified before
the corresponding stream bytes are drained, so a rejected candidate can still
use those original bytes. A corrupt incoming block is terminal because a stream
cannot reread an earlier interval.

## Bounds and Shutdown

Lookahead retains at most `concurrency` range responses, with one additional
fallback response. Each response is bounded by `max_range_size`, giving a retained
response limit of `(concurrency + 1) * max_range_size`. Defaults retain four 1 MiB
range responses plus one 1 MiB fallback response. Current encoded and decoded
block buffers, codec workspace, and metadata are separate from this bound. An
encoded block larger than the range limit is assembled from multiple requests.

The first worker failure stops scheduling, records its error, and cancels the
source. Dropping an unfinished fetcher clears queued jobs, cancels active reads,
and joins every worker. A successful fetcher joins without canceling the source,
allowing the parser to continue with subsequent payloads and framing.

A backend must provide cancellation or bounded blocking I/O for prompt shutdown.
HTTP cancellation interrupts backoff immediately. An active network operation
ends at its current deadline; cancellation does not forcibly close its socket.

## HTTP Behavior

Each installation has one shared `ureq::Agent` connection pool. Completed bodies
are read through EOF to return connections to the pool. Range responses must
match the requested interval and probed resource length. HTTP content encoding
must preserve byte offsets. A strong ETag, when supplied, pins subsequent
requests through `If-Match`.

HTTP sources probe range support by default. Unsupported servers fall back to
the original full response; `--disable-range-queries` opens a forward stream.
Range-capable sources use the block plan's actual reuse opportunities instead
of choosing a strategy from slot-index availability.

DNS, connection setup, request transmission, and response headers have 60-second
timeouts. `--http-stall-timeout` bounds each idle socket read, defaulting to
60 seconds; it does not change connection or request deadlines. The transport
checks cancellation beneath TLS, including reads that process TLS control records.
A response that keeps receiving data can exceed the stall timeout. The wrapper uses
ureq's unversioned transport API and must be checked when upgrading ureq.

Both `update install` and `apps install` accept these positive tuning values:

| Option | Default | Meaning |
| --- | --- | --- |
| `--http-stall-timeout` | `60` | Maximum idle socket wait in seconds. |
| `--http-max-range-size` | `1MiB` | Maximum bytes per range, accepting byte units. |
| `--http-concurrency` | `4` | Maximum range workers and requests in flight. |

The concurrency setting also sizes the HTTP connection pool. Range settings have
no effect on a full streaming response.

Transient HTTP failures use the existing retry and backoff settings. A range
reader resumes only its own remaining interval. Consecutive retry counts reset
when body bytes arrive. Invalid intervals and changed representations are
terminal.

Download statistics count actual response-body bytes, including probes and
replacement reads. Skipped bytes are the resource length minus the union of
consumed intervals, recorded when readers close. These counters differ from the
parser's logical installation position.

See the [HTTP benchmark](../benchmarks/http-fetch/README.md) for reproducible
throughput workloads and baseline comparisons.
