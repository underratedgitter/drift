# Drift: architecture and delivery plan

## Scope and decision

The first slice uses a Rust service plus strict TypeScript/React. Rust was installed
at the user's request; no Xcode project or xcodebuild is used. Cargo uses the host
toolchain/linker. A loopback browser UI makes the engine testable without a native
wrapper. The next iteration adds an Electron desktop shell, packaged without an
Xcode workflow, plus an HTTPS phone browser app. Tauri remains a lower-idle-memory
future option, not implemented. This is a functional LAN preview, not a production
release. See PHONE-DESKTOP-PLAN.md and PHONE-SETUP.md for the new trust boundary.

Dependencies: Tokio/Axum (MIT), rustls (Apache-2.0/ISC/MIT), ring (mixed permissive
licenses; inspect bundled notices), rcgen (MIT/Apache-2.0), reqwest (MIT/Apache-2.0),
mdns-sd (Apache-2.0/MIT), redb (MIT/Apache-2.0), React, qrcode and Vite (MIT),
TypeScript (Apache-2.0). Cargo.lock and package-lock.json pin transitive versions.
Electron/electron-builder (MIT) and @noble/hashes (MIT) support the new clients.
The packaging-only global-agent override upgrades its legacy logging dependency
chain to avoid the vulnerable sprintf-js path; audit and re-test it on updates.
Dependency/license audits and security updates are release gates. No hosted backend
or account database is needed for LAN operation. redb is a local, transactional,
pure-Rust embedded metadata store; it avoids a SQLite C build dependency.

## Boundaries

* `engine/src/pairing.rs`: expiring code and certificate-bound challenge proofs.
* `engine/src/discovery.rs`: untrusted DNS-SD advertisements, `_drift._tcp`.
* `engine/src/transport.rs`: TLS bootstrap and authenticated peer requests.
* `engine/src/lib.rs`: sessions, approvals, bounded chunk transfer and recovery.
* `engine/src/storage.rs`: private partial files, transactional metadata, publication.
* `engine/src/web.rs`: loopback-only control API, guarded by a random capability.
* `engine/src/phone.rs`: separate TLS phone gateway, one-use enrollment and scoped offers.
* `desktop/`: sandboxed Electron shell, user-driven file selection and engine lifecycle.
* `ui/phone-download.worker.ts`: bounded OPFS writes, checkpoints and SHA-256 off the UI thread.
* `ui/`: React views, no arbitrary filesystem or peer-network authority.

Transport selection target: authenticated local TLS, then ICE direct data
channel, then TURN. Only local TLS is implemented in this slice. A LAN address
is not an internet rendezvous mechanism. Future remote signaling must use WSS,
opaque expiring rendezvous IDs, quotas and no file payloads. WebRTC DTLS identity
must bind to pairing; TURN sees ciphertext only. Remote is not silently routed
through a web upload service. Mobile needs native file-provider/document-picker
integration, not a desktop shell. iOS cannot promise arbitrary filesystem access,
always-on discovery or indefinite background transfers. Android background work
needs a user-visible foreground service and scoped storage permissions.
The implemented browser companion uses foreground file pickers, a home-screen
manifest and private browser storage, not native background privileges. It cannot
silently trust self-signed TLS; the owner manually installs the exported public
CA/profile. The issuer private key is never persisted. The retained leaf key is
private-file protected but not yet OS-keychain protected.

## LAN protocol v1

Each process creates an ephemeral TLS certificate. An invitation contains v1,
local address, port, temporary 120-bit code and expiry; QR codes hold the same
invitation, never permanent credentials. Code lifetime: five minutes. Manual
entry requires an address, port and the same 30-character hexadecimal code (120
bits). The UI groups it for readability. Six-digit codes
are deferred until a reviewed PAKE/rendezvous implementation exists.

The client chooses a random nonce. `/proof` returns HMAC-SHA256(code,
the JSON tuple `(1,"server",fingerprint,nonce,"")`). The client compares against the **observed** TLS
certificate fingerprint before sending any metadata. `/pair` uses a distinct
client HMAC domain and nonce; nonces are consumed once. Invitations are consumed
on a successful request. A random session capability is returned over pinned
TLS. The receiver explicitly approves, then the sender explicitly confirms.
Every subsequent request pins the certificate and presents the capability.
No device advertisement, hostname or display name is an authentication factor.

Manifests contain relative paths, safe-integer sizes and BLAKE3 digests, not
absolute source paths. Receiver approval is per manifest, not just per device.
Eight MiB chunks are committed in order. Receiver reports durable offsets and
rejects out-of-order writes. Retries query receiver offsets; partial files and
history survive process restarts; session secrets deliberately do not. After a
restart devices pair again and reoffer the same manifest to resume. If only the
sender restarted, the receiver first revokes the previous incoming session;
another session cannot silently take over an active manifest. Pairing is directional
in this preview; reverse sending requires pairing in the other direction. Whole-file
SHA-256 is checked before no-clobber publication. No application file-size cap
below JS safe integer/filesystem limits; file count and manifest size are bounded
to prevent metadata exhaustion. Source files are hashed and streamed, never
buffered whole. Partial-file flush per chunk favors reliable recovery over peak
disk throughput; benchmark before tuning. Initial concurrency limit is one send
per engine, with serialized receiving writes. Browser selection stages files to a
private outbox first, requiring additional disk space and delaying first-byte
delivery. Native path-based selection is a future speed improvement. Directory
pickers and Chromium/WebKit directory drag-and-drop preserve nested files; empty
directories are not transferred. UI polling reports measured committed-byte deltas, not
invented speeds. No AirDrop performance claim is made.

## Threat model

Protect against hostile LAN peers, forged discovery, MITM, replay, brute force,
path traversal, duplicate/case-colliding paths, unintended overwrite, and malformed
or oversized requests. HMAC certificate binding prevents a TLS-terminating MITM
from relaying a valid proof for its own certificate. Codes are 120 random bits;
proof endpoints are rate limited. Pairing approval never means future file
approval. Files stay in an app-private directory until verified. Publishing uses
hard links into an unpublished directory followed by rename, with a unique
content-derived transfer directory; existing differing files are never
overwritten. The data root rejects symlinks and is mode 0700 on Unix. Do not run
with attacker-writable ancestor or receive directories. A privileged
local attacker, compromised OS, malicious file contents, traffic analysis, and
denial of service beyond bounded requests are outside the security guarantee.
No code execution or auto-opening of received content. No telemetry.

Credentials: ephemeral session capabilities only, in memory; no persistent trust
keys yet. Trusted-device autoaccept and OS credential-store integration remain
deferred together. History is local metadata and can be cleared. Logging must
never include invitations, session capabilities or codes. The private control URL
is printed once to the local terminal so its owner can open the UI; treat terminal
output as a secret. UI capabilities remain in tab memory, not cookies or local
storage. The loopback API has no CORS support and requires Authorization, preventing
cross-site forms and DNS-rebinding pages from making authorized requests. TLS validation bypass
is allowed only during certificate-bound proof bootstrap, never for file payloads.

## Verification and release gates

Automated tests exercise real TLS sockets and filesystem writes on this host:
pairing, approvals, expiry/replay, unauthorized writes, integrity, resume,
path validation and conflicts. mDNS must also be exercised across real devices;
its behavior is network/environment dependent. No cross-OS claims without
hardware testing. No claimed throughput until measured. Remote, mobile, signing,
notarization, sandbox entitlements, OS credential storage, update security and
independent crypto review are release gates, not completed features.
