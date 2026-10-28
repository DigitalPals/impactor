# Sources in this fork

Sources are a local desktop feature. They use Impactor's existing Apple ID signing,
installer options, device installation, and provisioning refresh. No server or
additional account service is required.

## Add and browse a source

1. Open **Sources** in the top bar and paste an HTTPS source URL. For example:

   ```text
   https://github.com/mrdrvt99/Altstore-Repository/blob/main/youmod.json?utm_source=chatgpt.com
   ```

2. Choose **Add source**. Impactor fetches and validates the catalog before saving
   it, then shows its name, app count, last successful check, and diagnostics.
   The example above is resolved to:

   ```text
   https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/youmod.json
   ```

3. Add another feed, for example:

   ```text
   https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/ytmusic.json
   ```

4. Use **All sources**, a source's name, or the search field to find an app by
   name, developer, or original bundle ID. Open it for its description, source,
   release date, size, OS requirements, release notes, and history. Descriptions
   are inert text. Artwork is optional; missing/failed artwork uses a placeholder.
5. Sources and successfully parsed catalogs survive restarts. **Refresh** checks
   one enabled source; **Check releases** checks all enabled sources. Failed
   refreshes keep the previous catalog and show the error and last successful
   timestamp. Startup checks only sources whose last success is over six hours
   old. There is no recurring source polling.

You can browse before signing in or connecting a device. Tab/Shift+Tab move focus,
Enter/Space activate focused buttons, PageUp/PageDown scroll, and the window can
be resized. Ctrl+L (Command+L on macOS) focuses the source URL; Ctrl+F/Command+F
focuses search. Escape returns from app details.
The import button returns to the existing local IPA importer.

## Install and update

Connect/trust a device and select it in the existing device menu. Impactor reads
its actual OS/platform, chooses the first compatible release in publisher order,
and displays incompatible history entries. Choose **Install** to download,
validate, and open the **existing installer options screen**. Signing begins only
when you use that screen's Install button. Sign in through the normal Apple ID
dialog if needed. You can cancel a download and retry it.

Downloads support GitHub release redirects and suffixless CDN URLs. Before
signing, Impactor checks HTTP status, declared size, SHA-256 when supplied, ZIP
integrity, the main app's original bundle ID, version/build, supported platform,
device family, and minimum OS. A mismatch includes expected/actual values. Ask
the source publisher to fix inaccurate feed metadata rather than bypassing it.
The download original remains separate from the mutable IPA staging copy.

After device installation succeeds, Impactor records source provenance in
`source-installs.json`. Failed installs and exports do not create provenance.
The record is scoped to source URL, original bundle ID, device, and Apple
account/team. It includes the artifact checksum, actual signed identity, and
existing installer options. Account tokens/passwords/cookies are not stored in
source files or sent in source, artwork, or download requests.

New source installs default to a source-scoped signed bundle identifier:
`<original>.s<source-key-prefix>.<Apple-team>`. This keeps different sources'
same-bundle variants independent. A custom identifier is checked for conflicts
against provenance and the connected device before signing. These source
installations require Apple ID signing; the local IPA workflow retains its
existing signing modes.

Select the same device and Apple account in Settings to see **Updates** and its
count. Known installed identities are reconciled with the device when possible.
An unrelated app with the same original ID is never enough to mark a source app
installed. Updates restore the previous signed identity and installer options;
they do not uninstall first. Changing the target, account/team, signing mode, or
installed identity requires returning to Sources, or produces an explicit
identity conflict. Restore missing saved tweak/icon/entitlement files or adjust
the options before installing. Removing/disabling a source cancels its pending
requests and stops its update checks.

There are two separate operations:

* **Check releases / Updates** fetch catalog metadata and offer new IPAs. Every
  installation requires user action; there are no automatic version upgrades.
* **Auto Refresh / tray refresh** renews provisioning for the already installed
  release, using the existing refresh store. It never consults a source or fetches
  a new release. Removing/disabling a source leaves those refresh records/assets
  and installed apps intact.

New provisioning refresh entries retain their own Apple account and original
team, so apps signed with different accounts/teams on one device renew under
the correct identity. Older refresh records keep their existing device-level
account fallback.

Updates intentionally use conservative evidence. Numeric dotted versions/builds
are compared numerically (so `1.10` follows `1.9`); a lower version is never
offered as an update. For other version formats, publisher order establishes
newness only if the installed release still appears in history. Dates and notes
do not establish newness. Same-version/build artifact changes and uncomparable
versions missing from history are labeled **changed release**, outside the
Updates count. Selecting an older history entry is explicitly labeled **Install
selected older release** and uses the same installer. Matching versions/builds
and artifact identity do not repeatedly produce updates. A URL-only artifact
change is ambiguous; matching publisher SHA-256 values allow expiring signed
URLs to rotate without producing false updates.

## Supported source format and URL rules

The compatibility target is [AltStore Classic's source specification](https://faq.altstore.io/developers/make-a-source)
and SideStore-style IPA feeds, with ordinary JSON and JSON5. Modern
`apps[].versions[]` records take precedence. Flat app `version`/`downloadURL`
fields are used only when no modern record is valid, with a legacy diagnostic.
The source name and apps array, app name/bundle ID, and release version/download
URL are required. Optional metadata, missing artwork, unknown `apiVersion`, news,
featured apps, and permissions do not prevent browsing. String/object screenshot
forms and iPhone/iPad screenshot groups are normalized. Relative URLs resolve
against the fetched feed location. Invalid neighboring records are skipped with
diagnostics; an empty catalog is valid and clearly identified.

Source identity is a hash of the canonical URL, never the feed's `identifier` or
`sourceURL`. Different sources can share identifiers and bundle IDs. Duplicate
bundle IDs within one feed are diagnosed and ignored after the first app.
Equivalent simple GitHub blob/raw addresses deduplicate. GitHub presentation and
tracking parameters are removed during conversion; arbitrary source queries and
signed download queries are preserved. Ambiguous slash-containing GitHub ref/path
splits are resolved through GitHub's public contents API, accepting only one
verified file. If that is unavailable, rate limited, or ambiguous, use the Raw
URL. Impactor never guesses a ref or silently replaces another source's identity.

AltStore PAL/ADP marketplace packages and paid-source authentication are
unsupported and displayed as unavailable. Feed sources require HTTPS; loopback
HTTP is supported for local catalogs/tests. Artifact/artwork URLs may use
HTTP(S). Redirects are bounded to eight and HTTPS downgrade redirects are
rejected. TLS certificate verification remains enabled. Limits are 8 MiB per
feed, 2 MiB per remote artwork, 4 GiB per download, four concurrent network
transfers, and 8 GiB total uncompressed archive data. Unsafe paths, symlinks,
multiple main apps, and corrupt ZIP entries are rejected before extraction.

## Storage and recovery

Files use the existing Impactor data directory:

* Linux/macOS: `$XDG_CONFIG_HOME/PlumeImpactor`, falling back to
  `~/.config/PlumeImpactor`.
* Windows: `%APPDATA%\PlumeImpactor`.

`sources.json` contains only source configuration/cached catalogs, and
`source-installs.json` contains nonsecret installation provenance. Both use
atomic replacement. Existing `accounts.json` and provisioning refresh records
remain compatible; their writes also use atomic replacement. Corrupt or future
source stores are reported rather than silently overwritten. Back up damaged
files before repairing them. Temporary downloads live in `source-downloads`, use
random app-owned filenames, and are removed after cancellation/failure/staging.
Provisioning assets remain in `refresh_store` under the existing refresh logic.

## Linux build and run

On Debian/Ubuntu, install the desktop build prerequisites and a current stable
Rust toolchain (tested with Rust 1.96):

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev libgtk-3-dev \
  libayatana-appindicator3-dev libxkbcommon-dev libxkbcommon-x11-0 libxdo-dev libvulkan1 usbmuxd
rustup update stable
cd ~/Code/impactor
cargo +stable build --locked --bins --workspace
./target/debug/plumeimpactor
```

For software rendering, prefix the executable with `ICED_BACKEND=tiny-skia`.
For packaged Linux binaries, the existing command is
`make linux PROFILE=release`; AppImage packaging uses `APPIMAGE=1`. Do not run
the desktop as root. Device discovery requires usbmuxd, USB access, and trusting
the computer on the device. Provisioning still has Apple's existing account,
entitlement, and free-account limits.

Deterministic tests use sanitized pinned publisher fixtures, local HTTP
responses, and small generated IPA ZIPs; no account, physical device, external
feed, or third-party IPA is needed:

```sh
cargo +stable test --locked -p plume_source -p plume_store -p plumeimpactor
cargo +stable check --locked --workspace
cargo +stable clippy --locked --workspace --all-targets
```

The existing release workflow has Linux x86_64/aarch64, Windows x86/x86_64/ARM64,
and macOS Intel/ARM64 jobs. Native macOS/Windows and real Apple provisioning,
entitlement/signing, device installation, and a real in-place upgrade still need
their respective hosts/devices. Offline tests validate the handoff and persistence
boundaries; they cannot certify Apple services or a physical phone.
