# Releasing a new version

Cutlass updates itself from **0.1.2 onward**. Anyone still on 0.1.1 or earlier
has to download once by hand — those builds shipped before the updater existed
and have no way to learn a new version is out.

## The one thing that silently breaks updates

The build must be signed, or the installer it produces **cannot be offered as
an update to anyone**. Nothing fails loudly when you forget: the build succeeds,
the installer works, and existing users simply never hear about it.

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.cutlass-keys/updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=
```

`scripts/release-manifest.sh` refuses to continue if the signature is missing,
which is the guard against shipping an invisible release.

### Back up that key

`~/.cutlass-keys/updater.key` is **not in this repo and cannot be regenerated.**
Its public half is compiled into every copy of Cutlass already installed, and
those copies will only accept updates signed by this exact key.

Lose it and no future version can ever reach anyone automatically — every user
has to be told to reinstall by hand, forever. Keep a copy in the password
manager alongside the licensing key.

## Steps

1. **Bump the version** in `apps/desktop/src-tauri/tauri.conf.json`. That file
   is the single source of truth — the app reads it for crash reports, and the
   updater compares against it.

2. **Update the docs** that name the version or the download file: the README
   badge *and* its Install section, `beta/RELEASE_NOTES.md`, and
   `site/index.html`. The filename appears in more places than the badge does;
   it has been missed before.

3. **Build the trial installer** with the signing variables above set:

   ```bash
   cd apps/desktop && npm run tauri build
   ```

   Confirm it produced *both* files:

   ```bash
   ls target/release/bundle/nsis/
   # Cutlass_<version>_x64-setup.exe
   # Cutlass_<version>_x64-setup.exe.sig   <- without this there is no update
   ```

4. **Run the export harnesses** against any short clip with audio:

   ```bash
   scripts/run-harnesses.sh path/to/clip.mp4
   ```

   `cargo test` compiles examples but never runs them, so these check nothing
   unless someone runs them deliberately. The gap-deadlock regression hid in
   that blind spot for months and is now a real test; what's left here needs
   real footage and a real encoder, so it can't be.

5. **Write the manifest:**

   ```bash
   scripts/release-manifest.sh <version> "one line of release notes"
   ```

   Those notes are shown in the update banner inside the app, so write them for
   a user, not a changelog.

6. **Tag and publish the release**, then upload the installer *and*
   `latest.json`. The updater only ever reads `latest.json`; an installer
   attached without it is invisible.

7. **Verify it from outside** — the only test that counts:

   ```bash
   scripts/release-manifest.sh --verify <version>
   ```

   This checks the manifest is published, advertises the right version, names
   an installer that actually downloads, and carries a real signature. Every
   one of those failures is invisible from inside the app — a wrong asset name
   looks exactly like "no update available", so nobody finds out until the
   release after this one also fails to land.

   **Do not rename the installer on the way to the release.** 0.1.1 was
   published as `Cutlass-0.1.1-trial-x64-setup.exe`, a hand-rename of the built
   file; that convention is dropped. The manifest points at the name the build
   produced, and a rename breaks every update silently.

## The Creator edition does not update

The owner build (`--features owner`) never checks. The endpoint serves the
public trial build, so an owner build that updated would replace the only
unrestricted copy of Cutlass with a trial one. Rebuild it by hand when you want
the newest code:

```bash
cd apps/desktop && npm run tauri build -- --features owner
```

**Never publish that installer**, to a GitHub release or anywhere else.

### Move it before you do anything else

Both builds write to **the same path and the same filename**:

```
target/release/bundle/nsis/Cutlass_<version>_x64-setup.exe
```

So building the Creator edition silently overwrites the trial installer you
published, and leaves the trial's `.sig` sitting next to it as if they belonged
together. Re-run an upload from that folder afterwards — a `--clobber` to fix a
typo in the release notes, say — and you have published your own unrestricted
build as the public download, under a name that looks exactly right.

Move it out of the way as soon as it finishes:

```bash
mkdir -p installers
mv "target/release/bundle/nsis/Cutlass_<version>_x64-setup.exe" \
   "installers/Cutlass_<version>_CREATOR-EDITION_x64-setup.exe"
```

`installers/` is gitignored, so it can't be committed either. If you still need
the trial installer afterwards, rebuild it — or keep a copy before you start.

Check which one you have by feature flag, never by searching the binary for
text. `cfg!(feature = "owner")` is a runtime boolean, so **both builds contain
all the same strings** — grepping for "Creator edition" finds it in the trial
build too, and has fooled me before:

```bash
cargo tree -p cutlass-desktop -f "{p} [{f}]" --depth 0   # trial: []
```

### Don't sign the Creator edition

Build it without `TAURI_SIGNING_PRIVATE_KEY` set. Tauri prints an error at the
signing step and still produces the installer, which is the outcome you want:
the Creator edition never checks for updates, so the signature would do nothing
useful, and a *signed* owner installer is a genuinely dangerous thing to have
lying around — one accidental upload away from replacing every user's trial
with your unrestricted build, verifying perfectly on the way in.
