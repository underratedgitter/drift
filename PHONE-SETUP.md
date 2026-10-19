# Use Drift as an app, with your phone

## Open the Mac app — no Terminal needed

The local Intel Mac build is at **`release/mac/Drift.app`** inside `drift/`.
Open it in Finder. You can move it to Applications yourself. The Rust engine and
interface are bundled; Node and Cargo are not needed to run it.
Closing its window hides it and keeps transfers running. Use **Drift → Quit
Drift** or Cmd+Q to stop the engine.

The app uses its own Electron userData directory, normally
`~/Library/Application Support/Drift/engine`; Settings shows the actual receive
path. It does not overwrite/import the CLI's `~/.drift` history. If another Drift
instance uses phone port 9443, close that instance normally first.

This is a locally built **unsigned, unnotarized** app, not a public distribution
release. The build here is macOS Intel only. Other architectures and operating
systems need their own builds and hardware tests. No Xcode/xcodebuild workflow
was used. Electron adds idle memory compared with the Rust engine alone.

## One-time phone trust setup

The Android/iPhone client is an **installable browser app**, not an APK or IPA.
Open **Devices** in Drift on the Mac. Phone access has a separate HTTPS listener;
never share the Mac's private loopback control URL.

Browsers cannot authenticate a private LAN HTTPS server without a trusted
certificate. An HTTP page could be replaced by a hostile network peer. Drift
does not use HTTP file transfer or tell you to ignore TLS warnings.

1. **iPhone/iPad:** export `Drift-Phone.mobileconfig` on the Mac. Transfer it using
   a channel you already trust, such as a cable or AirDrop. Open it on the phone.
   Install under **Settings → General → VPN & Device Management**, then enable
   Drift under **Settings → General → About → Certificate Trust Settings**.
2. **Android:** export `Drift-Phone-CA.crt` and transfer it using a trusted channel.
   Install under **Settings → Security/Privacy → Encryption & credentials →
   Install a CA certificate**. Manufacturer labels vary. Managed/work phones may
   prohibit installing CAs; Drift does not bypass that restriction.
3. Before trusting it, compare its **SHA-256 fingerprint** with the value on the
   Mac. Do not install certificates from strangers or fetch replacements over
   unauthenticated HTTP.

Installing a root certificate is a security-sensitive OS action. Drift never
does it automatically. Its CA issuing key is discarded after signing the Mac's
server certificate, so the retained leaf key cannot mint certificates for other
sites. Nevertheless, remove obsolete Drift roots/profiles. The leaf private key
is stored in the private Mac data directory (0600 on Unix); OS credential-store
integration for that TLS key remains unfinished.

## Pair and send

1. Keep both devices on the same non-isolated LAN/Wi-Fi. Allow local-network or
   incoming-connection prompts for Drift/Safari/Chrome where required.
2. In the Mac's Devices page select **New phone invitation**.
3. Scan the **phone QR** with the system camera and open its HTTPS link in Safari
   (iPhone) or Chrome (Android). This is different from desktop pairing.
4. Name the phone and select **Request connection**. Approve on the Mac, then
   select **Confirm connection** on the phone.
5. **Phone → Mac:** choose files and select **Send / resume**. Review and approve
   the file list on the Mac's Send & receive page.
6. **Mac → phone:** choose files with the native picker, select the phone in
   **Send to**, and send. On the phone choose **Receive / resume**. After SHA-256
   verification, use **Save file** or **Share / Save to Files** to keep them
   outside private browser storage.

Desktop native-picker files stream directly from their source paths. Browser
selection and desktop drag-and-drop still stage a copy. Symlinks are rejected
rather than followed outside selected directories.

## Add the phone app to the home screen

* **Android Chrome:** menu → Install / Add to Home screen.
* **iPhone Safari:** Share → Add to Home Screen.

No App Store, Google account or Apple account is required. Reopening needs a
fresh phone invitation. Session capabilities are not saved in cookies,
localStorage, service-worker caches or permanent QR codes. Revoke incoming
sessions in Devices on the Mac.

## Limits

* Keep the phone page foregrounded and the screen awake. Home-screen web apps
  do not gain native background-transfer privileges.
* Use recent Safari/Chrome with OPFS synchronous private-file access in dedicated
  workers. Unsupported browsers return an error, not a whole-memory or unverified
  fallback. Files must fit actual available browser storage quota.
* Downloads use bounded worker buffers, in-place private-file writes, flush
  checkpoints, incremental SHA-256 and prefix rehashing on resume. Save/share
  exports individual files, not arbitrary destination directory trees.
* Retry interrupted jobs with Send / resume or Receive / resume. After a Mac
  restart, pair again and reoffer/reselect the same files. Old phone offers are
  not restored automatically. If only a phone tab restarted, revoke its old
  incoming session before re-pairing to resume uploads.
* A changed Mac IPv4 address or expired certificate requires fresh trust setup.
  Guest Wi-Fi client isolation can prevent direct connectivity. Internet and
  direct phone-to-phone transfer are not implemented.
* Browser testing is not physical-device certification. Certificate installation,
  OS prompts, camera scans, home-screen launch, storage quotas and background
  suspension still require real Android/iPhone validation.

### Safari/WebKit test limitation on this build host

Playwright supplies a frozen WebKit build for macOS 14. Here it hangs creating
even an empty page, with no Drift code or server involved. Mobile options and
headed/headless mode did not resolve it. `node scripts/probe-webkit.mjs` is a
15-second environment diagnostic. On a working host, opt into the full test with
`DRIFT_TEST_WEBKIT=1 npx playwright test tests/ui/desktop-phone.spec.ts -g WebKit`.
The normal suite explicitly skips that WebKit case on this host; it is **not**
counted as an iPhone/Safari pass. Chromium phone-client and Mac app testing are
separate from this blocked check.

## Rebuild

```sh
npm ci
npm run desktop:dev
```

Package on the build host with `npm run desktop:package`. This builds Rust release
mode and bundles the downloaded Electron runtime, without Xcode. Public releases
need separate signing, notarization and external security review.
