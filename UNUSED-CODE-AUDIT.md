# Unused-code audit — before native platform work

Scope: the existing Drift application, before adding native Android/iOS clients.
The AC-controller parent application was excluded and preserved unchanged.

## Findings

* TypeScript strict checks, including noUnusedLocals/noUnusedParameters: passed.
* Rust Clippy across all targets with warnings denied: passed before migration.
* Knip 6.41.0's initial scan reported four files, not unused functions:
  `desktop/main.cjs`, `desktop/preload.cjs`, `public/phone-sw.js`, and
  `scripts/probe-webkit.mjs`. All are legitimate external entry points: Electron,
  its isolated preload, a registered service worker, and a documented diagnostic.
  They were retained and explicitly declared in knip.json rather than deleted.
  `rustup`, macOS `plutil`, and `lipo` are declared as external SDK tools, not npm packages; new native
  build commands are checked without treating that prerequisite as an unlisted
  npm binary. Native platform compilation is recorded separately from this JS audit.
* Removed the redundant HeaderMap parameter and `let _ = headers` binding from
  the staging handler. Authorization remains in its existing route middleware.

Compiler/linter checks cannot prove every public Rust API or dynamic callback is
used, nor prove arbitrary lines unreachable. Router registrations, native ABI
entry points, workers, lifecycle Drop implementations and test/tool entry points
require manual call-site review. No broad deletion or speculative cleanup was done.

## Repository separation

Drift is its own local repository at `~/Documents/repos/drift`. Excluded
dependencies, generated bundles, build caches, release apps and test artifacts.
The new repository is local only; no GitHub publication is authorized.

## Repeatable commands

```sh
npm run typecheck
npm run lint
npm exec --yes --package=knip@6.41.0 -- knip --no-progress
cargo clippy --manifest-path engine/Cargo.toml --all-targets -- -D warnings
```

Re-run after new entry points are added; do not hide real findings with global
ignore patterns. Generated platform build outputs must not be audited as source.
