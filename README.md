# Drift

Send files and folders directly between your Macs, over your own network.
Encrypted, verified on arrival, no account and no cloud.

- **Tap to pair.** Your other Macs running Drift appear under *Nearby devices*.
  Click one, check that both screens show the same six digits, and accept.
- **Pair once.** Paired Macs stay paired after restarts and when their network
  addresses change, and either one can send to the other.
- **Your own Macs just work.** Files from Macs you mark as yours arrive without
  a prompt. Anyone else's file list waits for your approval.
- **Files land where you look.** Received files go to **Downloads › Drift** with
  their own names. Taken names become "photo (2).jpg". Nothing is replaced.
- **Big folders, safely.** Thousands of files or tens of gigabytes. Transfers
  resume after interruptions, and every file is checked with BLAKE3 before it appears.
- **Stays out of the way.** Close the window and Drift keeps receiving from the
  menu bar, with a notification when something arrives.

Drift is a side project for macOS 11 or later, as a universal app for Apple
Silicon and Intel. It is not available for iPhone, iPad, Windows or Android.

## Install

Download the disk image from [Releases](https://github.com/underratedgitter/drift/releases)
(or the `Drift-macOS` artifact of a recent CI run), open it and drag **Drift** to
**Applications**.

The app is ad-hoc signed, not notarized by Apple, so macOS blocks it the first
time. Open it once with **right-click › Open** and confirm. On macOS 15 or later,
open **System Settings › Privacy & Security** and choose **Open Anyway**.

On first launch macOS asks two questions. Allow both:

- **Find devices on your local network.** Drift needs this to see and reach your other Macs.
- **Access your Downloads folder.** Received files are saved in Downloads › Drift.

## Use

1. Open Drift on both Macs, on the same Wi-Fi or wired network.
2. On one Mac, click the other one under **Nearby devices**. Both show a six-digit
   code. If they match, click **Approve** on the other Mac. Leave **This is one of
   my devices** checked for your own Macs.
3. Drop files or folders on the window, or click **Choose files** / **Choose folder**,
   then **Send files**.

If the other Mac never appears, for example on guest Wi-Fi that isolates
devices, use **Pair a device**. Copy the invitation on one Mac and paste it on
the other (Universal Clipboard works), or type its address and code.

Paired Macs are listed under **Devices**, where you can choose whether a Mac's
files arrive automatically, or forget it on both Macs.

## Build from source

Requirements: macOS with Xcode Command Line Tools, Node 22.12+ and stable Rust.

```sh
npm ci
npm run app:dev            # run the app with live UI reload
npm run app:build          # universal Drift.app and .dmg in target/universal-apple-darwin/release/bundle
```

`app:build` needs both Rust targets:
`rustup target add aarch64-apple-darwin x86_64-apple-darwin`.

The same engine also runs without the app, with its interface in a browser on
the same Mac. This is useful for development and the browser tests:

```sh
npm run build
cargo run -p drift-engine -- --name "Test Mac" --ui dist/ui --receive ~/Downloads/Drift
```

Open the private `http://127.0.0.1:9418/#…` URL it prints. The part after `#`
is a secret for this Mac only; do not share it.

## Checks

```sh
npm run typecheck && npm run lint
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace     # engine over real TLS sockets, plus the app's IPC
npx playwright install chromium && npm run test:ui
```

CI runs all of this on macOS for every push and pull request and builds the
universal disk image. Pushing a `v*` tag also publishes that image as a release.

## How it works

```
engine/      Rust transfer engine: identity, pairing, TLS transfers, storage
src-tauri/   the Mac app (Tauri 2): runs the engine in-process, menu bar,
             notifications, file pickers and drag and drop
ui/          the interface (React + TypeScript), shared by the app and the
             browser mode
tests/ui/    browser tests that drive two real engines end to end
research/    the Jev usage study behind the feature priorities
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the protocol,
[SECURITY.md](SECURITY.md) for the threat model and its tests, and
[PERFORMANCE.md](PERFORMANCE.md) for measurements.

## Why these features

Feature priorities came from a usage study with TypeSafe's Jev model
(`node scripts/jev-usage-study.mjs`, results in `research/`). Saving files
where people look, keeping devices paired, and accepting your own devices'
files without a prompt ranked highest. These are model judgments, not user
research.

## License

MIT
