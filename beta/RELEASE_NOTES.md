# Cutlass 0.1.8 — it tells you when an update doesn't land

**Cut sharper. Own everything.**

Windows 10/11 · 64-bit. Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

## Download

**Cutlass_0.1.8_x64-setup.exe** (Windows 10/11, 64-bit) — attached below.

Already on 0.1.2 or later? You don't need this page. Cutlass will offer the update itself.

Code-signed: Windows shows the publisher as **Isaiah Aniemeka**. SmartScreen may still warn while a new release builds reputation — **More info → Run anyway**.

## What's new

This is a small release with one fix in it, and it is worth explaining why.

### Cutlass now notices when an update didn't install

Testing the 0.1.6 → 0.1.7 update end to end for the first time, it failed — twice — and reported success both times. Security software was stopping the downloaded installer. Nobody would have known: the app closed, came back, and looked completely normal, just on the old version.

That happens because of how Windows updates work. Cutlass hands the installer to Windows and closes itself in the same instant, so there is no Cutlass left running to be told if the installer is then blocked. It simply starts up again on the version it already had.

So it now checks on the way back in. If you asked for an update and you are still on the old version afterwards, Cutlass says so, tells you the likely reason, and gives you a button to download it yourself:

> **Cutlass 0.1.9 didn't install.** You're still on 0.1.8. Security software often stops an installer it hasn't seen before — downloading it yourself usually works.

It says it once, not every time you open the app.

**This only helps from 0.1.8 onwards.** The version doing the checking has to already be installed, so if an update from 0.1.7 or earlier fails, it still fails quietly. That is the honest reason to take this one.

### If an update ever does get blocked

Download the installer from the releases page and run it. It installs straight over your existing copy — your projects, settings and licence are untouched. Nothing needs uninstalling first.

## Fixed

- Nothing else. Everything in this release is either the above or work you can't see: the collaboration relay got its first tests, the test suite now runs on GitHub instead of only on one machine, and the pricing notes were corrected to describe what is actually sold.

## Known limits

- Windows only.
- SmartScreen reputation is still building, and so is the reputation the installer needs with antivirus software. Both improve as more releases are signed with the same certificate.
- 4K exports are slow by nature. Export at 1080p for long videos.
- Collaboration isn't available in this build.
- The offline moment finder has no comprehension — it can't tell you *why* a moment works the way the AI can. A good fallback, not a replacement.

## Something wrong?

**Help → Send beta feedback** inside Cutlass, or open an issue. Please include your **graphics card**, and for anything export-related the clip length and the file size you got.
