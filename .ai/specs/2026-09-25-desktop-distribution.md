# Desktop distribution — where cezar desktop is published and how it updates

**Status:** design + workflow in place, no release cut yet. 2026-09-25. Companion to
`2026-09-25-self-update-and-desktop.md` (the shell itself and the managed install).

## TLDR

The desktop shell (`packages/desktop`, Tauri 2) is published as **signed installers on GitHub
Releases**, one release per shell version under a `desktop-v<version>` tag, built by
`.github/workflows/desktop-release.yml`. Everything downstream — the landing page's Download
button, a Homebrew cask, winget, the shell's own auto-update — is a pointer to that release
page. The shell ships **rarely and on its own version** (`0.1.0` today, decoupled from cezar's
`0.11.x`): it contains no cezar code, because the cockpit and server come from
`~/.cezar/versions` and update from inside the cockpit.

## Two things update on two cadences

| What | Contains | Updated by | How often |
| --- | --- | --- | --- |
| **cezar** (`@open-mercato/cezar`) | server + cockpit | the cockpit's version chip → dialog, or `cezar update` (managed install, spec self-update) | every release / nightly |
| **cezar desktop** (`packages/desktop`) | window, title bar, sidecar supervisor, splash, icon | Tauri's updater plugin polling `desktop-latest.json` on GitHub Releases | a few times a year |

The shell must ship when: the shell changes (window, supervisor, splash), a Tauri/webview
security fix lands, or the icon/name/signing identity changes. Never for a cezar release.

## The release: GitHub Releases, tag `desktop-v<version>`

`desktop-release.yml` runs on a `desktop-v*` tag push (or manual dispatch with a version) and:

1. **prepare** — verifies the tag equals `packages/desktop/src-tauri/tauri.conf.json`'s
   `version` (a mismatched tag fails before any build) and creates a draft release.
2. **build** (matrix) — macOS Apple silicon, macOS Intel, Windows x64, Linux x64, through
   `tauri-apps/tauri-action`. Each job uploads the versioned bundle AND a **stable, versionless
   copy** plus its `.sha256`:

   | Platform | Stable asset |
   | --- | --- |
   | macOS (arm64) | `cezar-macos-aarch64.dmg` |
   | macOS (x64) | `cezar-macos-x86_64.dmg` |
   | Windows | `cezar-windows-x86_64-setup.exe` (NSIS) |
   | Linux | `cezar-linux-x86_64.AppImage` (+ `.deb` versioned) |

   Stable names are the contract: `https://github.com/open-mercato/cezar/releases/latest/download/<asset>`
   always resolves to the newest shell, so the landing page and package-manager manifests never
   change when a version ships.
3. **publish** — renames tauri-action's `latest.json` to **`desktop-latest.json`** (the name
   the shells poll; namespaced so a future cezar manifest on the same page cannot collide),
   flips the draft to published with `--latest=false` (cezar's `v*` releases stay the repo's
   "latest"), and writes a summary with the stable links and loud warnings for anything unsigned.

`createUpdaterArtifacts` is passed **only in CI** (`--config`), never in the checked-in
config, so a local `tauri build` does not demand the signing key.

## Signing — the real gate on "easy install"

Secret-gated, all-or-nothing per platform, and the workflow degrades to unsigned builds with a
warning rather than failing — unsigned builds are fine for a maintainer to test and NOT fit for
a Download button (Gatekeeper reports them "damaged"; SmartScreen blocks them).

| Secret | Used for |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) | updater artifacts + `desktop-latest.json`. Public key is in `tauri.conf.json` `plugins.updater.pubkey`. Generated with `npx tauri signer generate`; the private half lives in the maintainer's `~/.tauri/cezar-desktop.key` and in this secret — lose it and installed shells can never adopt another update. |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | Developer ID Application certificate (base64 .p12) for code signing |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarization (app-specific password) |
| Windows code signing | not wired yet — Azure Trusted Signing via `bundle.windows.signCommand` is the intended route; until then Windows builds are unsigned |

Apple Developer Program membership (99 USD/year) is the one purchase required before publishing
to anyone but maintainers.

## Shell auto-update

`tauri-plugin-updater` is initialised in `lib.rs`; once per launch, in the background, the shell
fetches `desktop-latest.json`, verifies the minisign signature against the embedded public key,
and installs silently — the new shell takes over on the **next** launch. It never restarts under
the user (only the cockpit's own updater does that, after asking), never blocks startup, and is
disabled in debug builds and by `CEZ_DESKTOP_NO_UPDATE=1`.

## Downstream pointers (in the order to add them)

1. **Landing page "Download"** — a static page (Cloudflare/GitHub Pages) with OS detection and
   the stable asset links; optionally reads the release JSON for the version. No backend.
2. **Homebrew cask** in an `open-mercato/homebrew-tap`: `brew install --cask open-mercato/tap/cezar`,
   `url` = the stable dmg link, `sha256` from the `.sha256` asset, `livecheck` on GitHub Releases.
3. **winget** manifest (community repo), automatable from the publish job.
4. **Flathub** later; the AppImage covers Linux until then.

## The shell ↔ cezar contract (a public surface)

Any shell version must run any cezar version, so these are frozen and listed in
`BACKWARD_COMPATIBILITY.md`: the managed entry path
`~/.cezar/versions/current/node_modules/@open-mercato/cezar/dist/index.js`; the launch
`serve --no-open --port <n>`; `CEZ_DESKTOP=1`, `CEZ_SUPERVISED=1`, `CEZ_SUPERVISOR_PID`;
exit status **75** = "relaunch me"; `GET /api/v1/health` as the readiness probe. Changing any
of them is the one case where the shell must ship BEFORE the cezar version that needs it.

## What the shell does on its own (supervisor duties)

- **First launch** with no managed install: installs the channel's newest cezar
  (`npm install --prefix` into `~/.cezar/versions/<v>`, manifest, `current` link — the same
  layout the cockpit's updater writes) with progress on the splash, then starts it. Node 20+
  is the one prerequisite; the failure page says so.
- **App menu → "Update cezar to latest…"**: the same install, then a relaunch. This is what
  makes a downgrade into a version that predates the cockpit's updater recoverable from the
  GUI — the shell never depends on the sidecar being able to update itself.
- **App menu → Versions**: one check item per install under `~/.cezar/versions`, the active
  one checked; picking another flips `current` and relaunches. With "Update to latest" this is
  the complete recovery set: any installed version, including a local build, is one click away
  from any other, whatever the running cockpit knows.
- **Legacy title strip**: a cockpit that predates the desktop-aware shell paints no strip and the
  traffic lights would sit on its brand row. The shell's init script waits for the app to
  render, and when no `data-slot="desktop-titlebar"` appears it injects a draggable 28px strip
  in the cockpit's own sidebar colours and insets the app shell — so every version looks right
  under the shell, not only the ones that know about it.
- **Port**: 4321 first (so `http://localhost:4321` works in a browser beside the app), the
  next few when busy, then any free port; the actual URL is on the app menu's
  "Open … in browser" item.
- The sidecar writes the `~/.cezar/bin` launchers on boot when they are missing, so a
  machine that only ever installed the app still gets `cezar` in a terminal (PATH hook is
  left to `cezar install`).

## Open items

- Windows signing, Flathub, universal macOS binary (two dmgs today).
- A `cezar desktop` version chip somewhere in the cockpit (the sidecar knows `CEZ_DESKTOP`,
  the shell version could ride along in an env var) so a bug report names both versions.
- Windows/Linux title bar: native today; the macOS overlay treatment needs a custom bar there.
