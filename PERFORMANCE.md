# Performance

All numbers below are engine-only measurements over TLS on loopback on one
Intel Mac. They include hashing on both sides, but not Wi-Fi, a second Mac or
the app window. They are not Mac-to-Mac or AirDrop comparisons.

## Goal, not an unmeasured claim

Optimize for disk/network capacity on direct local connections. “Faster than
every app” or “faster than AirDrop” cannot be guaranteed. AirDrop uses Apple
peer-to-peer Wi-Fi (AWDL); Drift currently uses the existing LAN. A congested
router or slow access point can dominate the result irrespective of code.

## Current engine

TLS over direct TCP; negotiated sixteen-MiB durable windows with an eight-MiB
legacy fallback, bounded buffers, no whole-file buffering.
LAN pairing accepts private, loopback and link-local IPv4 addresses only; the
transfer client disables HTTP proxies and redirects. No relay is used in LAN mode.
Startup address selection prefers a routed physical link over VPN and virtual
interfaces. If no local link is detected, the interface says so instead of
claiming the network is ready. After switching networks, quit and reopen Drift so
it advertises its new address; paired Macs then find it again by its identity.
Source reads now run one chunk ahead of the durable network upload. A queue slot
is reserved before allocating/reading, bounding application payload buffers to
two negotiated windows (32 MiB maximum application payload on the sender);
HTTP/TLS/OS buffers and metadata are additional. The receiver admits at most one
sixteen-MiB window-route body at a time; legacy routes retain their own limits.
Tokio's internal file-I/O buffers are also outside this payload-buffer bound.
Retries share immutable byte buffers rather than copying each payload per attempt.
This is disk/network overlap, not multiple unordered HTTP requests in flight.
One active outgoing send per engine, serialized incoming writes. Files are hashed
before sending and after receiving. Metadata commits and file syncs protect
offset recovery. Every receiver ACK still follows a file sync for the acknowledged window, so acknowledged bytes are on disk. Receive progress (offsets) is committed at most every 250 ms instead of per chunk; after a crash the recorded offset may lag and those bytes are re-sent, but it never claims unsynced data. Up to one unacknowledged window may need retransmission after a
failure. Negotiation is authenticated, memory is capped locally even if a peer
advertises a larger size, and old peers without `/features` retain eight-MiB
chunks. Legacy limits are not silently relaxed. Dropping a paused,
cancelled or interrupted send aborts its read-ahead producer. Publication and
delivery to Downloads › Drift use hard links, so verified files are never copied
again on the same volume.
Sender progress history is now persisted at roughly 250 ms intervals and file
boundaries, plus final/error/control transitions, instead of committing after
every chunk. Receiver disk syncs remain per chunk; offset commits are coalesced to 250 ms because per-chunk commits rewrote the whole transfer record and made many-file transfers quadratic. A fresh sender always resumes from
the receiver's durable offsets, not its potentially older progress display.
The initial one-MiB implementation was benchmarked before increasing the chunk
size, amortizing HTTP round trips and durable commit barriers without weakening
the acknowledgement contract. Metadata remains limited to two MiB, and at most
four peer requests enter the engine concurrently. The Mac app reads chosen and dropped
files in place; only the browser mode stages a local copy before sending. This
is a reliability-first implementation, not yet a tuned high-throughput protocol.

## Repeatable benchmark protocol

For the included engine-only experiment:

```sh
cargo test --release -p drift-engine --test integration --no-run
npm run bench
```

`DRIFT_BENCH_MIB=256` and `DRIFT_BENCH_RUNS=5` are the defaults. Set them in the
shell to change the experiment; no .env file or credentials are needed. This
creates synthetic temporary files and writes `BENCHMARK_RESULTS.json`.

### Measured on this host (macOS x86_64, Rust 1.99.1)

Five 256 MiB, release-mode, eight-MiB-chunk loopback runs:

| Metric | Measurement |
|---|---:|
| Median throughput | 48.56 MiB/s (SHA-256; 149-151 MiB/s after the 2026-10-20 switch to BLAKE3) |
| Median transfer time | 5.272 s |
| p95 transfer time (nearest rank; only five runs) | 5.583 s |
| Initial one-MiB-chunk isolated run | 13.90 MiB/s |

A separate `/usr/bin/time -l` run of the same eight-MiB-chunk fixture measured
48.42 MiB/s, 5.63 s total process wall time, 2.73 s user CPU + 0.46 s system CPU,
and 47,325,184 bytes (45.13 MiB) maximum resident memory. Resource figures include
**both Rust engines and the test harness in one process**, plus setup; they exclude
Chromium, staging and any native UI. They are not a per-device memory estimate.
The fixture is sparse and zero-filled, so this is not a realistic disk or network
benchmark. Pairing occurs before timing; scripted approval, sender hashing,
TLS transfer and receiver hashing are included. An earlier five-run experiment
before the published-file resend guard measured 50.39 MiB/s median; the table above
is the final guarded implementation. Raw final runs are in `BENCHMARK_RESULTS.json`.

For Mac-to-Mac measurements use the release app on both Macs, with the same
files, power state, network and disks for every run. Test 1, 10 and 50 GiB incompressible files, 10,000 small files and nested
folders. At least five runs per scenario; report median and p95. Measure separately:

* selection-to-first-byte and selection-to-verified-file wall time;
* sustained payload throughput, excluding pauses for human approval;
* CPU%, peak RSS, disk read/write volume and network bytes;
* recovery after Wi-Fi loss, sleep/wake and process termination;
* wired LAN, Wi-Fi 5/6/6E and congested access-point scenarios.

`/usr/bin/time -l` measures process resource usage for a reproducible headless
benchmark. No Mac-to-Mac or AirDrop results have been measured yet.

## Next optimization experiments

1. Pipeline several bounded windows instead of one HTTP round trip per window,
   with a strict memory budget and cumulative durable acknowledgements.
2. Hash in a blocking worker pool, and try hashing while sending without losing
   stable resume identities.
3. Tune for both very large files and thousands of small ones.

Publish before/after measurements for each change. Never weaken integrity,
approval, authentication or encryption to win a benchmark.

### Current-session optimization experiment

128 MiB sparse, zero-filled release-mode TLS-loopback transfers on this Intel
Mac, including pre-hashing, scripted approval, transfer and final verification:

| Variant | Runs | Median MiB/s |
|---|---:|---:|
| Before this change | 3 | 38.76 |
| Bounded read-ahead/shared retry buffers | 3 | 43.27 |
| Plus sender-progress checkpoint coalescing, final repeat | 5 | 45.25 |
| Plus negotiated 16 MiB durable windows, isolated repeat | 5 | 58.99 |

The eight-MiB checkpoint repeat was approximately 16.7% above the current-session
baseline; the isolated sixteen-MiB-window repeat was approximately 52.2% above it.
An earlier checkpoint-optimization batch reached 49.47 MiB/s median but did not
repeat at that rate; all raw trials are kept in `PIPELINE-BENCHMARK.json`.
The eight-MiB repeat ranged from 43.18 to 48.05 MiB/s; the isolated sixteen-MiB
repeat ranged from 53.59 to 60.24 MiB/s. Do not cherry-pick the earlier
51.31 MiB/s single run or compare this different-size fixture directly with the
older 256 MiB measurements above. Small sequential samples, cache/thermal effects
and host load limit confidence; no universal or Wi-Fi speed superiority is
established. 58.99 MiB/s is about 494.8 decimal Mbps **on localhost only**. A
prior loaded batch that overlapped other builds had a lower 41.77 MiB/s median;
it is kept in the raw data. Real Mac-to-Mac measurements are the next input
before further transport changes can be judged honestly.
