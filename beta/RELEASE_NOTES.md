# Cutlass 0.1.2 — clearer audio, and a way back

**Cut sharper. Own everything.**

Windows 10/11 · 64-bit. Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

## Download

**Cutlass_0.1.2_x64-setup.exe** (Windows 10/11, 64-bit) — attached below.

On first run, Windows SmartScreen may warn *"Windows protected your PC"* because this beta isn't code-signed yet. Click **More info → Run anyway**. Then launch **Cutlass** from the Start menu.

**This is the last version you'll have to download by hand.** From here, Cutlass tells you when there's a new version and updates itself.

## What's new

### Exported audio is clear again

Voice cleanup was taking 11–12 dB of real voice out along with the hiss, which is why exports came back sounding muffled — both your voice and the video's own audio. It now removes a fraction of that, and there's a **Cleanup strength** slider (Light / Normal / Strong) so you can decide how hard it works on your own recordings.

If you've exported anything that sounded dull since installing Cutlass, it's worth exporting it again.

### Cutlass updates itself

New versions arrive in the app. You get a quiet note that one's available, you choose when to download, and you choose when to restart — nothing interrupts an edit, and a running export is never disturbed.

### A crashed window no longer loses your work

If the interface ever fails, you now get a screen that explains what happened and offers to **save a recovery copy** before anything else. Your project lives outside the part that can crash, so the work is still there to rescue. Reloading reopens the project where you left it.

### It tells you when footage is missing

Opening a project whose video files aren't reachable — an external drive that isn't plugged in, most often — used to leave you with blank clips and no explanation. Cutlass now names the files it couldn't find and says what to check.

### Exports fail early instead of late

An export that can't fit on the drive now says so before it starts, rather than an hour in. Cutlass also cleans up the temporary files an interrupted export leaves behind, which at 4K could be tens of gigabytes sitting in your Videos folder.

## Fixed

- Naming a custom Look now uses a proper dialog, and works reliably.
- Projects record which version of Cutlass wrote them, so a file from a newer version says so instead of looking damaged.
- The Help menu and beta feedback reports show the version you're actually running.
- Licence checks no longer fail for everyone if a single request goes wrong.
- Long exports with a tiny gap between clips no longer stall — now covered by automatic tests so it can't come back.

## Known limits

- Windows only.
- Not code-signed yet, so SmartScreen will warn on first run.
- 4K exports are slow by nature — roughly real time for 4K60. Export at 1080p for long videos.
- Collaboration isn't available in this build.

## Something wrong?

**Help → Send beta feedback** inside Cutlass, or open an issue. Please include your **graphics card** and, for anything export-related, the clip length and the file size you got — those two answers have solved nearly every export bug so far.
