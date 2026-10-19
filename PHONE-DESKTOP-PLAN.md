# Desktop app and phone companion

## Scope

Deliver a clickable macOS app and a foreground phone web app for both Android
and iOS. Do not claim a native APK/IPA: Java/Android SDK are absent, and the user
explicitly prohibited Xcode. Electron is the desktop shell for this iteration;
it packages Chromium and the existing Rust engine without invoking Xcode. Its
idle memory footprint is higher than Tauri. The Rust transfer path remains
independent and unchanged for desktop-to-desktop transfers.

## Phone trust bootstrap

Never expose the loopback control token or desktop API on the LAN. A separate
HTTPS listener serves only phone assets and limited phone endpoints. Browsers
must perform normal certificate validation: no HTTP file transport, silent
self-signed bypass, or trusting a QR fingerprint using downloaded untrusted JS.

For a home LAN without a public domain, generate a local CA and a server leaf
certificate for the current IPv4 address. **Discard the CA private key**; persist
only the leaf private key, leaf chain and public CA certificate in the private
data directory. The user exports the public CA/profile from the Mac and transfers
it through an already-trusted channel to the phone, installs it, and enables
trust in OS settings. Do not fetch a replacement CA over unauthenticated HTTP.
The issuing key is not retained, so the stored server key cannot mint certificates
for other sites. This is not a claim that installing roots is risk-free: users
must verify its fingerprint and remove obsolete roots. IP changes/expiry require
a new trust setup. No OS trust stores are changed automatically.

Once trusted, scan a short-lived invitation QR containing the HTTPS origin and
an enrollment capability in the URL fragment. Exchange it once for an ephemeral
phone session. Require approval on the Mac and confirmation on the phone. The
phone capability authorizes only its uploads and explicitly offered downloads.
Revoking the incoming session cuts off both directions. No persistent autoaccept.

## Transfers

Phone → Mac: incrementally hash selected files, offer the manifest, receive
approval on Mac, stream bounded chunks through the existing engine, and verify
SHA-256 before publication. Re-offering identical files can resume saved offsets.

Mac → phone: select a native-authorized or staged batch and an approved phone.
Offer only that batch, require the phone user to approve it, and stream one-MiB
ranges into origin-private browser storage. A dedicated worker uses synchronous
in-place file access and eight-MiB flush checkpoints, avoiding repeated
copy-on-write rewrites of a growing file. Verify incremental SHA-256, then expose
a save/share action. Do not construct whole-file ArrayBuffers or silently fall
back to an unverifiable download. Browser storage quotas and OPFS support are
explicit limits; files must fit available browser storage. Phone folders are
preserved on upload where the browser supplies paths; download saves individual
files rather than recreating arbitrary device directories.

## Platform limits and validation

Keep the phone app in the foreground with the screen awake. Safari/iOS and mobile
Chrome can suspend network work when backgrounded. Add-to-home-screen provides
an app-like launch, not native background privileges. Use normal file pickers and
system save/share sheets. No permanent filesystem access is implied.

Automate limited-session authorization, enrollment expiry/replay/revocation,
upload/download integrity, resume and separation from desktop administration.
Run browser E2E against real HTTPS with a test-only certificate trust override
in the harness; never ship an override to the app. Package/launch the desktop
app and verify child-engine cleanup. Physical Android/iPhone certificate install,
camera scans, OPFS quota, home-screen launch, and background behavior remain
unverified until exercised on hardware.
