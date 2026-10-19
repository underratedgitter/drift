# Drift

Account-free, encrypted **LAN file transfer preview**, with a Rust engine and a
React interface.

**Native installers and mobile builds:** see [native/README.md](native/README.md).
Development Android APK, Intel macOS DMG and Windows x64 NSIS build commands are
available. iOS source/Xcode project is prepared but no IPA has been built; full
Xcode and device signing are still required. These are not production-certified releases.

**Optional Android/iPhone browser companion:** see
[PHONE-SETUP.md](PHONE-SETUP.md). The generated Mac app is `release/mac/Drift.app`;
phone setup is in Devices. A one-time, explicit certificate-trust setup is required
for browser clients only, not the embedded native Android/iOS pairing flow.
The application never bypasses certificate errors.

## Run

Prerequisites: Node 22.12+ (or Node 24), stable Rust/Cargo and the host platform's
normal Rust build prerequisites. Desktop builds do not require a full Xcode app;
native iOS builds do. Android needs the JDK/SDK described in the native guide.

```sh
cd drift
npm ci
npm run build
cargo run --manifest-path engine/Cargo.toml -- --name "My laptop" --ui dist/ui
```

If Cargo is not on PATH yet, run `. "$HOME/.cargo/env"` first. Open the private
`http://127.0.0.1:9418/#…` URL printed in your terminal. The fragment is a local
control secret, **not** the invitation to give another device. Reloading the tab
requires reopening that private URL. Do not share it.

Start Drift independently on both devices, on the same LAN. Allow the Drift binary
through the local firewall if prompted. mDNS needs multicast UDP 5353; file
transfer uses the TCP port displayed under pairing. Guest Wi-Fi client isolation
can block both discovery and direct connections. No public port forwarding.
This preview uses IPv4 LAN addresses; IPv6-only networks are not supported yet.

1. On the receiving device, choose **Pair a device**.
2. On the sending device, select a discovered device and enter its temporary code,
   or paste its invitation. Address and port entry is available when mDNS fails.
3. Approve the incoming connection on the receiver.
4. On the sender, open **Devices** and select **Confirm connection**.
5. Choose files or a folder, select the paired receiver, then select **Send files**.
6. Review and approve the file list on the receiver. Files appear only after
   SHA-256 verification, under the location displayed in Settings.

Pairing is directional in this preview. To send in the opposite direction,
generate an invitation on the other device and repeat pairing in reverse.

The QR holds the same expiring invitation as the manual flow. This preview
generates QR codes but does not access a camera; paste externally scanned QR
contents into the invitation field. There are no permanent credentials in QR codes.

## Recovery, storage and privacy

* Pause stops at a chunk boundary. Retry with **Send / resume files** for the
  currently selected batch; the receiver approves again and reports saved offsets.
* After a restart, pair again and reselect the identical files to resume. Files
  must have the same relative paths and bytes. History and committed offsets persist;
  session keys and browser-selected batches do not.
  If only the sender restarted, revoke its previous incoming connection in the
  receiver's Devices screen before retrying through a newly paired session.
* After switching to a different network, restart Drift so its invitation and
  mDNS advertisement use the new interface address, then pair again. Seamless
  network migration and physical sleep/wake testing remain unfinished.
* Every received manifest has its own content-derived directory. Duplicate names
  in different subdirectories are preserved; case collisions and file/directory
  collisions are rejected. Overwrite is deliberately not available yet.
* `--data /path` changes the private data directory (default `~/.drift`). Outbox
  staging requires a local copy of selected files. Partials and outbox copies are
  retained for recovery, even after cancellation. Close Drift before manually
  deleting obsolete `outbox/`, `partial/` or `publishing/` contents. Do not edit
  `history.redb`; use a fresh data directory for a full local reset.
* Clear history removes completed/cancelled metadata, not files. There is no
  telemetry, cloud account, cloud storage, or persistent trusted-device list.
* Notifications are optional browser notifications; OS/browser support varies.
* Files are not automatically opened or executed. Treat received content as untrusted.

CLI options: `--help`, `--host 192.168.1.12`, `--port 9518`, `--ui-port 9418`,
`--phone-port 9443`, `--no-phone`,
`--data DIRECTORY`, `--name NAME`, `--ui dist/ui`, `--no-discovery`.
The control API binds only to loopback; never reverse-proxy it onto the LAN.

## Development checks

```sh
npm run typecheck
npm run lint
npm run build
cargo fmt --manifest-path engine/Cargo.toml -- --check
cargo clippy --manifest-path engine/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path engine/Cargo.toml -- --nocapture
npx playwright install chromium webkit
npm run test:ui
```

Tests use actual HTTPS listeners, random certificates, two engine instances,
filesystem writes and a transactional embedded database. Test output labels any
loopback timing explicitly: it is not Wi-Fi throughput or an AirDrop comparison.

## Status and limits

See [TASKS.md](TASKS.md), [ARCHITECTURE.md](ARCHITECTURE.md), and
[PERFORMANCE.md](PERFORMANCE.md). This is not the complete requested product.
The clickable Mac app and phone browser companion are implemented; see
[PHONE-SETUP.md](PHONE-SETUP.md) for the explicit certificate-trust step and limits.
Not implemented: Tauri, remote ICE/STUN/TURN/signaling, native APK/IPA clients,
in-app camera scanning, six-digit PAKE pairing, persistent identities/OS keychain,
custom receive-folder picker, configurable overwrite, background OS integration,
automatic updates, signing, or verified Windows/Linux builds. No relay deployment
is provided because no remote relay implementation exists yet. No fake remote
or trusted-device controls are presented as working.
