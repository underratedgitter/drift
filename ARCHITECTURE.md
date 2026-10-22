# Drift architecture

Drift is a macOS app built with Tauri 2. A Rust transfer engine runs inside the
app process; the React interface reaches it through Tauri IPC. Nothing is hosted:
two Macs talk to each other directly over TLS on the local network.

## Components

| Path | Responsibility |
|---|---|
| `engine/src/identity.rs` | One persistent TLS key pair per installation (`identity.json`, mode 0600) and the remembered listener port. A device *is* its certificate's SHA-256 fingerprint. |
| `engine/src/pairing.rs` | One-use 120-bit invitation codes with certificate-bound HMAC proofs; commitments and six-digit short codes for tap-to-pair. |
| `engine/src/transport.rs` | Outgoing connections. After pairing, every connection is pinned to the peer's fingerprint by a custom rustls verifier that still checks handshake signatures but ignores hostnames, so remembered Macs survive address changes. |
| `engine/src/lib.rs` | The peer API (pairing, offers, chunks, finish, control, forget), authentication by capability, remembered devices and address following. |
| `engine/src/storage.rs` | Transactional metadata (redb), partial files, verification, and delivery into the visible receive folder. |
| `engine/src/pipeline.rs` | Bounded read-ahead of source files. |
| `engine/src/discovery.rs` | mDNS advertisement and browsing of `_drift._tcp`. Advertisements are untrusted hints. |
| `engine/src/selection.rs` | Expands files and folders chosen with the system picker or dropped on the window. Symlinks are refused. |
| `engine/src/web.rs` | The control API used by the interface: in process for the app (`app_router`), or loopback HTTP behind a random capability for the browser mode. |
| `src-tauri/` | The Mac app: in-process engine, IPC commands, file pickers, drag and drop, menu bar item, notifications, single instance, `drift://` links. |
| `ui/` | The interface. `ui/transport.ts` hides whether it runs in the app or a browser. |

The web view never names a file on disk. Paths come from the system picker or
from drag and drop, on the Rust side, so a compromised page could not choose
what to send. Its Tauri capability only allows listening to app events.

## Pairing

Pairing happens once per pair of Macs, with one of two methods. Either way the
receiving Mac's user approves it, and then both Macs remember each other.

**Tap to pair (nearby Macs).** Mac B taps Mac A in the Nearby list:

1. B sends `/introduce` with its name, its identity (`DeviceHello`: fingerprint,
   port, a capability A will use to call B) and a commitment `H(nonce_B)`.
2. A stores the request and replies with a fresh `nonce_A`.
3. B reveals `nonce_B` to the certificate it saw in step 1. A checks the commitment.
4. Both compute a six-digit code from both fingerprints and both nonces. The
   person compares the two screens and approves on A.

The commitment stops a device in the middle from steering the codes to match:
when it must answer B's commitment it does not yet know `nonce_B`. A mismatch
needs a person to accept it, and requests are rate limited (20 a minute, 8
waiting, 2 minutes each).

**Invitation code (no discovery).** A shows a one-use invitation: address, port
and a 120-bit code, valid for five minutes. B fetches `/proof`, an HMAC over A's
*observed* certificate fingerprint and B's nonce, so a TLS-terminating
middleman's certificate fails the check. B then pins that certificate and posts
`/pair` with its own proof and `DeviceHello`.

After approval, A stores B as a `TrustedDevice` (fingerprint, name, last address,
the capability to call B, and a hash of the capability B presents to A). B
learns of the approval by polling and stores A the same way. There is no
separate confirmation step on B: starting the pairing was its consent.
Forgetting a device removes it on both sides when the other side is reachable.

Remembered devices follow address changes from their mDNS advertisements and
from their own authenticated requests. Because connections are pinned to the
fingerprint, a wrong address can only fail to connect; it cannot redirect files.

## Transfers

- A manifest lists relative paths, sizes and BLAKE3 digests, never absolute paths.
- From a device marked as the user's own, an offer starts receiving at once.
  Otherwise the receiver approves the file list.
- Data moves in negotiated 16 MiB durable windows. 8 MiB chunks remain for
  older peers. The receiver syncs each window before acknowledging it.
- Offsets persist, so retrying the same files resumes after interruptions or
  restarts. Offering the same files again after completion delivers a numbered copy.
- Verified files are first published atomically into `received/<id>` in the app's
  private data. They are then delivered to Downloads › Drift under their own names, with
  " (2)" suffixes on conflicts. Hard links, or `create_new` copies across
  volumes, make sure nothing existing is replaced or merged into. The chosen
  names are saved before any file moves, so an interrupted delivery resumes into
  the same names. An empty `received/<id>` stays as the marker that the manifest
  arrived.
- One outgoing send at a time per Mac; incoming writes are serialized.

## Threat model

Protected: hostile devices on the LAN, forged discovery, man-in-the-middle
during and after pairing, replayed or guessed codes, path traversal,
case-colliding or reserved file names, overwriting existing files, oversized
metadata, and other paired devices touching transfers they do not own.

Not protected: a compromised Mac or same-user malware, malicious file contents
(Drift never opens or runs received files), traffic analysis, and denial of
service beyond the request and size bounds. Pairing is a custom protocol that
has not had an external review.

No telemetry, accounts, cloud storage or internet relay. The control API is
in process for the app; the browser mode binds only to loopback.
