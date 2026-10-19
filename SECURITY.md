# Security and verification notes

This is an unaudited LAN preview. Do not treat passing tests as independent
cryptographic review or use it to receive untrusted content automatically.
The threat model and protocol are in [ARCHITECTURE.md](ARCHITECTURE.md).

## Invariants and test evidence

`engine/tests/integration.rs` exercises actual TLS sockets and filesystem state:

| Invariant | Evidence |
|---|---|
| Wrong code cannot authenticate TLS endpoint | `tls_pairing_requires_both_approvals_and_rejects_wrong_code` |
| Proof binds observed certificate, role and fresh nonce | `pairing_expiry_replay_and_certificate_binding` |
| Tokens expire, consume once and have a request limit | `pairing_expiry_replay_and_certificate_binding`, `invitation_qr_round_trip_and_rate_limit` |
| Both devices confirm before file offer; files need separate approval | `tls_pairing_requires_both_approvals_and_rejects_wrong_code` |
| Network and browser APIs require distinct random capabilities | `unauthenticated_network_and_browser_requests_are_rejected` |
| Paths cannot escape destination or collide with directory/Windows reserved names | `unsafe_filenames_and_conflicts_are_rejected` |
| Wrong offsets and writes after completion fail | `real_tls_folder_transfer_verifies_hash_and_rejects_bad_offsets` |
| SHA-256 failure never publishes files | `integrity_failure_never_publishes_files` |
| Clearing history and resending cannot truncate already-published hard links | `clearing_history_and_resending_never_truncates_published_hard_links` |
| Other sessions cannot inspect or take over active transfers | `pause_retry_and_session_isolation` |
| Confirmed chunks survive reopening storage | `persistent_offsets_survive_storage_restart` |
| A stopped TLS listener breaks the connection; fresh pairing resumes offsets | `disconnection_repair_and_resume_after_receiver_restart` |
| Two receiving transfers do not mix contents | `simultaneous_transfers_remain_isolated` |
| Sender cancellation remains cancelled and reaches receiver | `sender_cancel_is_preserved_and_propagated_to_receiver` |

Browser E2E tests exercise connection approval, manual code entry, folder picking,
file-list approval, real transfers, on-disk byte equality, protected control URLs,
keyboard activation, both themes and a narrow viewport. mDNS and the release
benchmark are explicit opt-in tests because they depend on network/resources.

## Known limitations / release blockers

Incoming pending/active receiving manifests are limited to 32, and phone
outgoing offers to 32, so even a paired client cannot grow pending metadata
indefinitely. Existing partial manifests can be reoffered within quota. This is
not a general disk-space reservation or multi-user DoS guarantee.

The new phone gateway mounts no desktop control routes. Tests in
`engine/tests/phone.rs` validate its generated CA with normal TLS verification,
enrollment expiry/replay, both-device approval, revocation, upload integrity,
scoped downloads, verification of every zero-byte file and native-import denial
without the desktop-only capability. The desktop renderer cannot name arbitrary
source paths: its preload only opens the OS picker, and main-process IPC checks
the sender/origin and supplies a separate secret for native batch registration.

Phone trust setup is manual. The CA private key is discarded; the retained leaf
key is mode 0600 under the private data root, not yet protected by OS credential
storage. Session/enrollment capabilities remain ephemeral. No production browser
TLS override or automatic root installation exists. E2E overrides are confined
to isolated test browser contexts and do not alter OS trust stores.

* Pairing is a custom high-entropy PSK certificate-binding protocol, not PAKE.
  Never shorten its random 120-bit code to a human password or six digits.
  Obtain external review before production distribution.
* Local host compromise, same-user malicious processes, browser extensions,
  filesystem tampering and attacker-writable ancestor directories are outside
  the guarantee. Use a private data directory on a local filesystem. Windows
  ACL hardening has not been independently validated.
* A hostile authenticated receiver can lie about its own verification. The
  sender cannot prove what a compromised receiving device writes to disk.
* Network DoS beyond request/concurrency/metadata bounds, disk-space quotas,
  free-space reservation and automated partial-file cleanup need more work.
* Self-signed certificates and session keys rotate on process restart. Persistent
  trusted-device keys, OS credential storage and trust revocation UI are not
  implemented; incoming ephemeral sessions can be revoked in Devices.
* No persistent autoaccept, file execution, cloud file storage, telemetry or
  internet transfer is enabled. Do not expose the loopback control service.
* Received files retain their content, not original permissions, ACLs, symlinks,
  extended attributes or executable flags. Empty directories are omitted.
* Desktop signing, installers, safe auto-updates, platform firewall integrations,
  physical network-switch/sleep-wake tests and Rust advisory auditing are release
  gates. npm audit alone does not audit Rust dependencies.
* Process/network restart recovery is tested; abrupt power-loss and filesystem
  crash-consistency guarantees have not been validated across filesystems.

Report suspected issues privately to the project owner. Do not include pairing
codes, private interface URLs, file contents or session capabilities in reports.
