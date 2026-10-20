# Performance plan

The recorded measurements below are from the earlier standalone Rust core, not
the new Electron shell or a physical phone. Native desktop picker selections
now bypass browser staging. Desktop drag-and-drop also imports OS-backed File
objects through the isolated preload/native capability instead of staging.
The CLI browser UI and mobile WebView file-input uploads still stage; direct
mobile source integration is not implemented by this optimization.
Phone uploads use four-MiB Blob chunks. Phone downloads use one-MiB ranges and a
dedicated worker with in-place OPFS writes/eight-MiB flush checkpoints to avoid
repeated copy-on-write copies of growing files. Phone throughput, browser quota,
Electron RSS and AirDrop comparisons have not been measured on hardware.

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
Startup address selection enumerates local interfaces rather than blindly using
the Internet default route: on phones it prefers hotspot links and excludes
cellular/VPN interfaces. This addresses hotspot invitations otherwise advertising
a cellular IP. Desktop selection prefers a usable routed local link. If no local
link is detected, the UI warns instead of claiming the network is ready. Restart
after changing networks; interface changes are not yet followed automatically.
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
cancelled or interrupted send aborts its read-ahead producer. Desktop publication
uses hard links; Android uses a verified private copy and atomic rename because
its sandbox denies hard links.
Sender progress history is now persisted at roughly 250 ms intervals and file
boundaries, plus final/error/control transitions, instead of committing after
every chunk. Receiver disk syncs remain per chunk; offset commits are coalesced to 250 ms because per-chunk commits rewrote the whole transfer record and made many-file transfers quadratic. A fresh sender always resumes from
the receiver's durable offsets, not its potentially older progress display.
The initial one-MiB implementation was benchmarked before increasing the chunk
size, amortizing HTTP round trips and durable commit barriers without weakening
the acknowledgement contract. Metadata remains limited to two MiB, and at most
four peer requests enter the engine concurrently. Browser staging adds a complete
local copy and pre-send delay. This is explicitly
a reliability-first implementation, not yet a tuned high-throughput protocol.

## Repeatable benchmark protocol

For the included engine-only experiment:

```sh
cargo test --release --manifest-path engine/Cargo.toml --test integration --no-run
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

Build release mode (`cargo build --release --manifest-path engine/Cargo.toml`).
Use the same sender, receiver, file set, power state, network and disk for all
apps. Test 1, 10 and 50 GiB incompressible files, 10,000 small files and nested
folders. At least five runs per scenario; report median and p95. Measure separately:

* selection-to-first-byte and selection-to-verified-file wall time;
* sustained payload throughput, excluding pauses for human approval;
* CPU%, peak RSS, disk read/write volume and network bytes;
* recovery after Wi-Fi loss, sleep/wake and process termination;
* wired LAN, Wi-Fi 5/6/6E and congested access-point scenarios.

macOS `/usr/bin/time -l` can measure process resource usage for a reproducible
headless benchmark. Linux can use `/usr/bin/time -v`; Windows needs equivalent
OS tooling. No cross-platform numbers or AirDrop results have been measured here.

## Next optimization experiments

1. Native mobile file handles: eliminate WebView outbox staging with scoped file authority.
2. Windowed streaming: pipeline several bounded chunks rather than an HTTP RTT
   per chunk. Keep a strict memory budget and cumulative durable acknowledgements.
3. Batch disk/metadata flushes at a documented recovery checkpoint interval;
   truncate to the last durable checkpoint on restart. Never acknowledge volatile
   bytes as durable just to improve a benchmark.
4. Hash in a blocking worker pool and experiment with inline sender hashing while
   preserving immutable resume identities. Benchmark SHA-256 hardware acceleration.
5. Tune concurrency against both large and small files; avoid starving foreground
   applications. Do not add compression for already compressed media by default.

Publish measured before/after results for each change. Do not weaken integrity,
approval, authentication or encryption to beat another application's speed.

## Mobile-hotspot comparison protocol

Blip's reported 300–400 Mbps is 37.5–50 MB/s (decimal), not 300–400 MB/s.
The current localhost results cannot establish that Drift beats it over Wi-Fi.
Use the same laptop and phone, connection direction, power state, source file,
hotspot band and distance for both apps. Keep the phone foregrounded/unlocked and
avoid cellular downloads, VPNs and unrelated traffic during the comparison.

* Prefer 5 GHz when both devices support it. On supported iPhones, disabling
  **Maximize Compatibility** permits the faster hotspot mode; it does not
  guarantee a particular link rate. On Android, choose the hotspot's 5 GHz band
  if offered. Use 6 GHz only if the hotspot, laptop, OS and region actually
  support that mode; Wi-Fi 6 branding alone does not imply 6 GHz.
* Connect the laptop to that phone's hotspot, then pair using Drift's invitation
  or manual private IP if multicast discovery is blocked. No relay, public port
  forwarding or application account is needed. Do not change device settings
  automatically or disable certificate validation.
* Test both directions with the same 1 GiB or larger incompressible file, at least
  five times each. Verify the received hash. Record median and p95 elapsed times,
  setup/hash time, sustained payload rate and total time until verified completion.
* Compute Mbps as `bytes * 8 / seconds / 1_000_000`; report MB/s separately.
  Record source staging/copy overhead, hotspot band and negotiated link rate.
  PHY link rate is not completed-file throughput.

Drift cannot force a hotspot into 5/6 GHz, bypass its radio/thermal limits or
guarantee a speed higher than Blip. Before/after engine-only experiments for the
bounded pipeline are recorded separately in `PIPELINE-BENCHMARK.json`; they are
not phone, hotspot, Windows, Apple Silicon or competitor measurements.

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
repeat at that rate; all raw trials are retained in `PIPELINE-BENCHMARK.json`.
The eight-MiB repeat ranged from 43.18 to 48.05 MiB/s; the isolated sixteen-MiB
repeat ranged from 53.59 to 60.24 MiB/s. Do not cherry-pick the earlier
51.31 MiB/s single run or compare this different-size fixture directly with the
older 256 MiB measurements above. Small sequential samples, cache/thermal effects
and host load limit confidence; no universal or hotspot speed superiority is
established. 58.99 MiB/s is about 494.8 decimal Mbps **on localhost only**, not
evidence of exceeding the user's 400 Mbps hotspot target. A prior loaded-window
batch overlapped platform builds and had a lower 41.77 MiB/s median; it is retained
in the raw data, not hidden. Real phone-to-laptop measurements remain the
next required input before further transport changes can be evaluated honestly.
