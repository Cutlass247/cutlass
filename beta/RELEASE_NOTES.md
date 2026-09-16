# Cutlass 0.1.7 — find your best moments with no internet

**Cut sharper. Own everything.**

Windows 10/11 · 64-bit. Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

## Download

**Cutlass_0.1.7_x64-setup.exe** (Windows 10/11, 64-bit) — attached below.

Already on 0.1.2 or later? You don't need this page. Cutlass will offer the update itself.

Code-signed: Windows shows the publisher as **Isaiah Aniemeka**. SmartScreen may still warn while a new release builds reputation — **More info → Run anyway**.

## What's new

### Find the best moments, offline

**Find the best moments** used to need a connection, and simply failed without one. It now falls back to a finder that runs entirely on your own machine — it reads the shape of your clip from the audio and the wording, picks the self-contained moments, and hands you the same list.

Set **Transcribe** to **Private** and the whole path is offline: import a recording, transcribe it on your machine, find the moments, cut them, burn captions, export. On a plane, on hotel wifi that went down, on a machine that never goes online at all.

The moments say where they came from. Found offline, they carry a note — the on-device finder scores audio and wording rather than reading your clip the way the AI does, so its picks are rougher, and you should know that rather than wonder why they got worse. Run it again when you're back online for the full pass.

Your licence keeps working offline too: **30 days** after its last check for a purchased copy, 3 days during the trial.

### We tightened how we describe Private mode

Our own docs said Private mode meant "100% on-device, no cloud at all" and that "airplane mode works." That was written before **Find the best moments** existed, and it wasn't quite true any more: the transcript was sent to be analysed even in Private mode.

Your **video** has never left your machine and still never does — that part was always accurate. But the wording overstated the rest, so we fixed it everywhere, including the hint under the Transcribe switch. As of this release the offline claim is true again, and this time it's tested.

## Fixed

- The Transcribe hint in Create and the tooltip in Studio both promised that nothing leaves your machine in Private mode. They now say what actually happens.

## Known limits

- Windows only.
- SmartScreen reputation is still building.
- 4K exports are slow by nature. Export at 1080p for long videos.
- Collaboration isn't available in this build.
- The offline finder has no comprehension — it can't tell you *why* a moment is funny the way the AI can. It's a good fallback, not a replacement.

## Something wrong?

**Help → Send beta feedback** inside Cutlass, or open an issue. Please include your **graphics card**, and for anything export-related the clip length and the file size you got.
