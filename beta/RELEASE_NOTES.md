# Cutlass 0.1.6 — imports that don't make you wait

**Cut sharper. Own everything.**

Windows 10/11 · 64-bit. Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

## Download

**Cutlass_0.1.6_x64-setup.exe** (Windows 10/11, 64-bit) — attached below.

Already on 0.1.2 or later? You don't need this page. Cutlass will offer the update itself.

Code-signed: Windows shows the publisher as **Isaiah Aniemeka**. SmartScreen may still warn while a new release builds reputation — **More info → Run anyway**.

## What's new

### Import several files at once

The Import button now takes as many files as you like. Clips appear in the bin one after another as each finishes, with the status counting them off, so you can start working on the first while the rest arrive. If a file can't be read you're told which one — the others still import.

### Screen recordings import straight away

Footage recorded at a variable frame rate — OBS captures, screen recordings, most phone video — has to be evened out before it can be cut, or the audio drifts away from the picture. Cutlass used to do that conversion **before showing you the clip**, so importing a long screen recording meant staring at a frozen window.

The clip now appears immediately and the conversion runs behind it. If you export before it's finished, Cutlass waits and tells you why rather than handing you a video that drifts.

### Long clips import about twice as fast

Cutlass was building twice as many preview thumbnails as it needed for the scrub strip. Same strip, half the work — most noticeable on clips over a minute, which were the slowest.

### It cleans up after itself

Every import left behind cached thumbnails — and for converted footage, a whole second copy of the video — and nothing ever deleted any of it. On the machine this was built on, ordinary use had already left 267 MB in the temp folder. Cutlass now clears anything it hasn't touched in two weeks.

## Fixed

- Failed collaboration connections said nothing at all; they now explain what went wrong.
- The export progress bar could be fed a bad value while preparing footage.
- Documentation no longer advertises collaboration, which isn't available in this build.

## Known limits

- Windows only.
- SmartScreen reputation is still building.
- 4K exports are slow by nature. Export at 1080p for long videos.
- Collaboration isn't available in this build.

## Something wrong?

**Help → Send beta feedback** inside Cutlass, or open an issue. Please include your **graphics card**, and for anything export-related the clip length and the file size you got.
