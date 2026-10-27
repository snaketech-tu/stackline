# Releasing swarmdash

Releases are built by GitHub Actions when a tag `v*` is pushed
(`.github/workflows/release.yml`). The workflow creates a draft release,
uploads the desktop installers and the single binaries, and publishes the
release once every file is there.

## One-time setup: the update signing key

Every download that swarmdash installs by itself is checked against a public
key that is built into the app. Create the key pair once, on your own
computer:

```sh
cd desktop
npm ci
npx tauri signer generate -w ~/.tauri/swarmdash.key
```

Then:

1. Add two repository secrets under *Settings → Secrets and variables → Actions*:
   - `TAURI_SIGNING_PRIVATE_KEY`: the contents of `~/.tauri/swarmdash.key`
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the password you chose
2. Put the contents of `~/.tauri/swarmdash.key.pub` into
   `crates/swarmdash/updater.pub` and into `plugins.updater.pubkey` in
   `desktop/src-tauri/tauri.conf.json`, then commit both. CI checks that
   they are the same.

Keep the private key and its password somewhere safe. Without them no future
release can be installed as an update, and users would have to download the
next version by hand.

## Every release

1. Set the new version in the root `Cargo.toml` (`[workspace.package]`). The
   desktop app and the binaries take their version from there.
2. Add a `## <version>` section at the top of `CHANGELOG.md`. It becomes the
   release notes and is what users see in the update dialog.
3. Commit, then tag and push:

   ```sh
   git tag v0.2.0
   git push origin main v0.2.0
   ```

The release workflow stops early if the tag and `Cargo.toml` disagree, if the
changelog has no section for the version, or if the signing key is missing.

## What gets published

| File | Used by |
| --- | --- |
| `swarmdash-<target>[.exe]` and `.sig` | `swarmdash serve`; it replaces itself with the file for its platform |
| `.msi`, `.exe` (NSIS), `.dmg`, `.app.tar.gz`, `.deb`, `.rpm`, `.AppImage` and their `.sig` | the desktop app and first-time installs |
| `latest.json` | the desktop app's updater |

The installers are not code-signed for Windows or notarized for macOS yet, so
the first start shows a warning there. Updates are still verified with the key
above.
