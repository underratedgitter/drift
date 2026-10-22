# Security

Drift has not had an independent security review. The protocol and threat model
are in [ARCHITECTURE.md](ARCHITECTURE.md). Passing tests are not a cryptographic
audit.

## Invariants and their tests

All of these run over real TLS sockets and the real filesystem
(`cargo test --workspace`):

| Invariant | Test |
|---|---|
| A wrong code cannot authenticate; proofs bind the observed certificate, role and a fresh nonce; codes expire and work once | `tls_pairing_requires_both_approvals_and_rejects_wrong_code`, `pairing_expiry_replay_and_certificate_binding` |
| Tap-to-pair codes depend on both identities and both nonces | `pairing::tests::short_codes_depend_on_every_input` |
| Both screens show the same code; one approval pairs both directions | `tapping_a_nearby_device_pairs_after_both_screens_show_the_same_code` |
| A reveal must match its commitment, and a request cannot be retried | `a_reveal_that_does_not_match_its_commitment_is_rejected_once_and_for_all` |
| A device whose identity differs from its advertisement is refused | `tapping_a_nearby_device_pairs_after_both_screens_show_the_same_code` |
| Paired devices are remembered across restarts with the same identity | `devices_stay_paired_both_ways_across_restarts_and_own_files_arrive_without_asking`, `identity::tests::identity_survives_restart_and_is_private` |
| Files from devices not marked as the user's own wait for approval | `files_from_other_peoples_devices_wait_for_approval` |
| Declining or forgetting removes trust on both sides | `declining_or_forgetting_removes_the_pairing_on_both_sides` |
| Delivery never replaces or merges into existing files | `folders_and_many_files_are_delivered_with_readable_names`, `storage::tests::readable_names_keep_extensions_and_skip_existing_files` |
| Network and control APIs require their own random capabilities | `unauthenticated_network_and_browser_requests_are_rejected` |
| The app's IPC rejects unknown methods, path traversal and non-byte uploads | `src-tauri: the_interface_reaches_the_engine_only_through_safe_ipc_requests` |
| Paths cannot escape, collide by case, or use reserved names | `unsafe_filenames_and_conflicts_are_rejected` |
| Choosing a folder never follows symlinks | `selection::tests::symlinks_are_refused_not_followed` |
| Wrong offsets and writes after completion fail | `real_tls_folder_transfer_verifies_hash_and_rejects_bad_offsets` |
| A failed checksum never publishes files | `integrity_failure_never_publishes_files` |
| Other devices cannot inspect or take over a transfer | `pause_retry_and_session_isolation`, `another_session_cannot_take_over_an_unfinished_transfer` |
| Acknowledged data and offsets survive restarts | `persistent_offsets_survive_storage_restart`, `disconnection_repair_and_resume_after_receiver_restart` |

## Limits

- The identity key is a file readable only by the user (mode 0600), not stored in the Keychain.
- A paired device's capability is a bearer secret, and stolen data from one Mac
  would let an attacker impersonate it to the other. Forget the device to revoke it.
- A hostile paired receiver can lie about what it verified or wrote.
- Pending incoming manifests are limited (32), but there is no disk-space
  reservation: a large approved transfer can fill the disk.
- Received files keep their contents, not permissions, extended attributes,
  symlinks or executable flags. Empty folders are not sent.
- Builds are ad-hoc signed and not notarized, and there is no automatic updater.

Report suspected issues privately to the project owner. Never include
invitation codes, private interface URLs, capabilities or file contents.
