# Releasing a new version

Cutlass updates itself. A build that shipped before the updater existed has no
way to learn a new version is out, so anyone on one has to download once by
hand — worth remembering if you ever hand someone an old installer.

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

   Editing `site/index.html` is only half of publishing it — see
   [The landing page is served from a different repo](#the-landing-page-is-served-from-a-different-repo).

3. **Build the trial installer** with the signing variables above set:

   ```bash
   cd apps/desktop && npm run tauri build
   ```

   **Check it is signed.** A signing step that fails produces an unsigned
   installer that looks identical until a user hits SmartScreen:

   ```powershell
   Get-AuthenticodeSignature "target\release\bundle\nsis\Cutlass_<version>_x64-setup.exe"
   # status must be Valid, signer must be CN=Isaiah Aniemeka
   ```

   Do **not** check `target\release\cutlass-desktop.exe`. It reads NotSigned
   even on a good build, because Tauri patches it with bundle metadata after
   signing. See `beta/CODE-SIGNING.md`.

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
   `latest.json` — **in that order, as two separate commands.**

   ```bash
   gh release upload v<version> "…/Cutlass_<version>_x64-setup.exe" --clobber
   gh release view v<version> --json assets --jq '.assets[].name'   # confirm
   gh release upload v<version> "…/latest.json" --clobber
   ```

   The updater only ever reads `latest.json`, so an installer attached without
   it is invisible — but the reverse is worse, and it has happened. A release
   went up with both in one command; GitHub answered **HTTP 500** partway, and
   the manifest landed while the 241 MB installer did not. Every installed copy
   was then told a new version existed and got a **404** trying to fetch it.

   `latest.json` is the switch that turns a release on. Throw it last, once
   there is something behind it. Uploaded in that order, a failed installer
   upload leaves the previous release serving happily and nobody ever sees a
   broken update.

7. **Verify it from outside** — the only test that counts:

   ```bash
   scripts/release-manifest.sh --verify <version>
   ```

   This checks the manifest is published, advertises the right version, names
   an installer that actually downloads, and carries a real signature. Every
   one of those failures is invisible from inside the app — a wrong asset name
   looks exactly like "no update available", so nobody finds out until the
   release after this one also fails to land.

   **A renamed installer needs a matching manifest.** An early release was
   published under a hand-renamed filename while the manifest still pointed at
   the name the build produced, which broke updating silently. If you rename
   the asset, rewrite the manifest's `url` to match it and verify the published
   URL resolves — the signature is over the file's contents, not its name, so
   renaming is safe as long as the manifest agrees.

8. **Hide the release it replaces**, so the page shows one download:

   ```bash
   gh release edit v<previous> --draft
   ```

   Last, not first — do it only once step 7 has confirmed the new release is
   actually reachable. Drafting the previous one before that leaves nothing
   downloadable if the new one turns out to be broken.

## The landing page is served from a different repo

**https://cutlass247.github.io/ is not published by this repository.** It is a
GitHub *user site*, served from `Cutlass247/Cutlass247.github.io`, which holds
a single `index.html`.

`site/index.html` here is the source. Pushing it does not publish it:

```bash
scripts/publish-site.sh          # copy site/index.html to the live page
scripts/publish-site.sh --verify # check the live page matches
```

This caught fire once already. Both addresses used to serve a copy of the page
— the root hand-edited, `/cutlass/` deployed from `site/` — and they drifted
for weeks without anyone noticing. The root went on showing a "See the AI in
action" button after it had been replaced by **Buy now — $49**, and went on
advertising collaboration after the feature was pulled from the build. Two
prices, two feature lists, decided by which link someone happened to have.

Now `/cutlass/` serves only a redirect (`site-redirect/index.html`), kept alive
because links to it are already out in the world — the Lemon Squeezy review
among them. And the Pages workflow runs `--verify` on every push that touches
`site/`, so **editing the page without publishing it fails a check** instead of
going quietly stale. A red `check-root-site` means exactly one thing: run the
publish script.

Publish *before* you push, and the check is green on the same commit.

`--verify` retries for two minutes rather than judging on one look, because
GitHub Pages takes a minute or two to serve a page you just published — and a
check that goes red every single release is a check people learn to ignore.
Forgetting to publish still fails, two minutes later.

## Only the latest release is public

Every release except the newest is a **draft**. Drafts are visible to accounts
with write access and invisible to everyone else, so the releases page shows
one download and nobody has to work out which build to take.

Nothing is deleted. Drafts keep their notes, their assets and their tag, and
the git tags all remain on the remote — `git ls-remote --tags origin` still
lists every one.

**After publishing a new release, draft the one it replaced:**

```bash
gh release edit v<previous> --draft
```

The consequence to know about: a drafted release's download URL returns **404**
to the public. If you have ever sent someone a direct link to a specific
installer, that link dies when you draft it.

## Rolling back a bad release

This is not hypothetical — a release once shipped an import regression and was
pulled the same day. Do it the moment you know, not after deciding whose fault it is.

Because older releases are drafts, un-hiding comes first. **One command, two
flags:**

```bash
gh release edit v<previous> --draft=false --latest
```

`--latest` alone is not enough: a draft cannot be latest, so without
`--draft=false` the command appears to work and changes nothing. Then check
what the world sees, which is the only test that counts:

```bash
scripts/release-manifest.sh --verify <previous>
```

It must report the previous version and a downloadable installer. Both the
download button and every installed copy's updater follow `latest`, so this one
command moves everybody.

Then mark the bad release so nobody installs it from the releases page — it is
still there, and its title will otherwise look like a normal build:

```bash
gh release edit v<bad> --title "Cutlass <bad> — superseded by <good>" --draft
```

Keep it rather than deleting it. A pulled release that quietly vanishes is
worse than one that says why it was pulled.

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

### Two different signatures — don't confuse them

Every build now carries two, doing unrelated jobs:

| | Proves | Comes from | Fails how |
|---|---|---|---|
| **Authenticode** | *who published it* — stops SmartScreen saying "unknown publisher" | Azure, via `signCommand` | build fails loudly |
| **Updater (minisign)** | *this is a genuine update* — installed copies accept it | `~/.cutlass-keys/updater.key` | silently absent |

**The Creator edition gets Authenticode but must NOT get the updater
signature.** Build it with `TAURI_SIGNING_PRIVATE_KEY` unset. Tauri prints an
error at that step and still produces the installer, which is the outcome you
want: the Creator edition never checks for updates, so the minisign signature
would do nothing useful — and a *minisign-signed* owner installer is genuinely
dangerous, one accidental upload from replacing every user's trial with your
unrestricted build and verifying perfectly on the way in.

Authenticode on the owner build is fine and desirable: it is your own machine,
and it means you don't fight SmartScreen on your own software.
