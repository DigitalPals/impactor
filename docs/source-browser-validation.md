# Source browser validation

Validated on Linux x86_64 on 2026-10-28, using Rust 1.96.0 in a disposable
Debian Bookworm build container. The local CI controller has no Impactor job;
the container was created under an `agent-resource` lease. Build products and
screenshots are under the ignored `target/` directory. No Apple credentials or
physical devices were used.

## Automated checks

* Formatting: rustfmt check passed on all changed/new Rust files, with
  `--edition 2024 --config skip_children=true` to avoid formatting unrelated
  upstream modules. `git diff --check` passed.
* `cargo check --workspace` passed.
* `cargo test --locked -p plume_source -p plume_store -p plumeimpactor` passed:
  27 source tests, one older-store/refresh ownership test, and eight GUI state /
  installer bridge tests, **36 total**.
* `cargo build --locked --bins --workspace` passed for the Linux GUI and CLI.
* `cargo clippy --locked --workspace --all-targets` passed with warnings in the
  existing workspace and integration code. No warnings are emitted for the new
  source crate / Sources screen / installer bridge modules.
* `cargo clippy --locked -p plume_source --all-targets -- -D warnings` passed.

The GUI test/build commands used `CARGO_BUILD_JOBS=4`,
`CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0` to bound container
resources. Tests use sanitized JSON fixtures and local mock HTTP responses;
generated IPAs contain only tiny test Info.plists and synthetic payload bytes.

Coverage includes exact blob/raw canonicalization and tracking queries, ambiguous
refs, meaningful/signed URL query bytes, JSON/JSON5, legacy fallback, malformed
neighbors, identifier/bundle collisions, release ordering and compatibility,
false updates, conditional 304s, redirects, HTTP/HTML errors, timeouts,
cancellation before and during a body, partial-file cleanup/retry, corrupt ZIPs,
traversal/path aliases, size/checksum and metadata mismatches, restart persistence,
stale completions, post-install device reconciliation, no account credentials in
requests, redirected HTTP downloads through the real installer bridge,
account/device/team scope, restoring signed identity/options, failed-install
provenance, and a newer mocked catalog that leaves the installed artifact/release
unchanged.

## Linux GUI smoke test

The rebuilt desktop was launched and restarted under Xvfb with tiny-skia and an
isolated `XDG_CONFIG_HOME`. Xdotool exercised the actual Iced UI:

1. Open Sources and paste the exact supplied YouMod GitHub blob URL.
2. Add it successfully; inspect the canonical raw URL, catalog name/app count,
   and current release metadata.
3. Add the publisher's `ytmusic.json`; both live catalogs appear in All sources.
4. Search by `dayanch96` and open YTMusicUltimate details; inspect developer,
   original bundle ID, release date/size, OS requirement, notes, and history.
   Install remains unavailable without a selected device.
5. Resize the window, use the scrolling content, rebuild/restart the GUI, and
   reopen Sources. Both cached catalogs and their original successful-check
   timestamps remain.
6. Disable YouMod, verify its disabled state and catalog persist, re-enable it,
   and remove YTMusicUltimate. The persisted source list reflects each action.
7. Use Tab/Enter from the import screen to open Sources, Ctrl+L/Enter to add a
   source, Ctrl+F to search, Tab/Enter to open details, Space to toggle a
   focused source checkbox, and PageUp/PageDown to scroll release notes.

The live catalogs were fetched only for research/manual smoke checks; their
versions and artifact URLs are not built into the feature or its tests.

## Remaining validation

The fork-only [checks run for `03a5165`](https://github.com/DigitalPals/impactor/actions/runs/40749020507)
passed native Linux and Windows workspace checks and all 34 tests present at
that commit. The macOS job never acquired a hosted runner and was cancelled
because GitHub reported ARM runner capacity constraints; it was retried, but no
macOS pass is claimed. The follow-up local checks above include two additional
regression tests. The existing release workflow's platform matrix was inspected;
no release was published and no upstream PR was created.

On-device validation still requires an Apple account and trusted device: real
certificate/provisioning requests, entitlements, signing, initial installation,
an in-place update under the same signed ID, and provisioning renewal after
disabling/removing its source. The automated suite verifies the boundaries and
failure behavior; it does not substitute for those Apple/device operations.
