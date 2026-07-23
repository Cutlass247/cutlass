# Cutlass — beta smoke test

Run the **packaged installer** (not the dev server) through the exact loop a
first tester will do. Everything here has passed in automated probes or the
browser mock — this pass is about the **native app on real footage**, which
those can't reach.

**Setup**
- Installer: `target/release/bundle/nsis/Cutlass_0.1.0_x64-setup.exe`
- Install it, then launch **Cutlass** from the Start menu (not `npm run tauri dev`).
- Have ready: one **talking-head clip** (you on camera, 30s+ with speech), and if
  possible a **long clip** (20 min+) and a **green-screen clip**. A `.cube` LUT
  file is a bonus.

**How to use this**
- Work top to bottom. Each item is: **do this → expect this**.
- 🔴 = native-only surface the mock/probes can't test — highest risk, look hard.
- When something is wrong (or just feels off), tell me the step number + what you
  saw. I'll fix on the spot and we re-run that section. Don't fix around it — a
  tester won't.

---

## A. The first five minutes (this must be flawless)

1. **Cold launch** — app opens to an empty project, no error dialog, window paints
   fully. → Clean editor, no red error banner.
2. 🔴 **Import** — click **+ Import**, pick your talking-head clip in the OS file
   dialog. → Dialog opens natively; after a moment the clip appears in the **media
   bin** (left panel) with a thumbnail. The app **does not freeze** while it works.
3. **Proxy/waveform** — the bin item shows a real thumbnail; selecting it doesn't
   spin. → No multi-second freeze; UI stays responsive.
4. 🔴 **Drag to timeline** — press-and-drag the bin item down onto a **V1** lane.
   → A ghost chip follows the cursor; releasing drops a clip where you let go.
   *(This is the one that only works in the native app — HTML5 drag is suppressed
   by WebView2, so it uses pointer drag. If nothing drops, stop and tell me.)*
5. **Playback** — press **Space**. → Full-motion video plays in the monitor (not a
   frozen/stuttering still), audio plays roughly in sync, playhead advances. Space
   again pauses.
6. **Scrub** — drag the playhead across the ruler. → Monitor updates smoothly as
   you drag; no long hangs.
7. **Transcription** — words appear in the **Transcript** tab on their own shortly
   after import (on-device). → Transcript populates; no "uploading" anywhere.
8. **Click-to-navigate** — click a word in the transcript. → Playhead jumps to that
   word in the video.
9. **Edit by deleting words** — select a sentence in the transcript and cut it. →
   The video **cuts**, the gap **ripples shut**, and it's **undoable** (Ctrl+Z
   brings it back).
10. **Smart cuts** — run remove-filler-words, then remove-silences. → Filler/dead
    air disappear in one action each; **Ctrl+Z** restores; playback of the result
    sounds clean.

## B. Grade & effects

11. **Open Effects tab** — with a clip selected. → You see **Looks** + effect
    sections (Color / Stylize / Motion / Mirror / Key / Audio) + LUT + Captions.
12. **Look thumbnails** — each Look chip shows *your current frame* graded. → Live
    thumbnails, visibly different (Cinematic/Noir/etc.), not just labels.
13. **Apply a Look** — click **Cinematic**. → The **monitor** changes immediately
    (color + vignette). Selecting a different Look re-grades live.
14. **Tune it** — in **Inspector**, open the color/Stylize groups; drag a slider,
    then **click a value and type** a number. → Monitor updates on drag *and* on
    typed commit; Esc cancels a typed edit.
15. 🔴 **LUT import** (if you have a `.cube`) — Effects → **Import LUT**, pick the
    file. → Native dialog opens; the monitor switches to a WebGL-graded canvas and
    the image changes. **Remove** reverts it.
16. **Auto-captions** — Effects → **Generate captions from transcript**. → Caption
    clips appear on **V2**; scrubbing shows captions burned over the video, timed
    to speech, positioned as a lower-third.
17. **Green screen** (if you have a keyed clip) — put it on **V2** over another
    clip, apply **Green screen**, tune Similarity. → The green drops out in the
    monitor and the lower track shows through.
18. **Motion** — apply **Ken Burns** (or Punch-in) to a clip. → Playing it shows a
    slow push/drift (Ken Burns is keyframed scale).

## C. Save, export, feedback (native dialogs)

19. **Save project** — Ctrl+S, choose a location. → Native save dialog; the
    "Unsaved" dot flips to "Saved"; a `.cutlass` file exists on disk.
20. **Reopen** — close the app, relaunch, **Open project**, pick the `.cutlass`. →
    Everything comes back: clips, cuts, grade, captions, tracks.
21. 🔴 **Export** — click **Export**. → Settings dialog opens (preset, name,
    location, format, res, fps, quality) with a live full-path preview. Pick a
    location and start.
22. **Progress** — a progress window shows a moving bar. → Bar advances; it doesn't
    sit frozen at 0%.
23. **Result** — on done, click **Open folder**. → Explorer opens with the file
    selected. **Play the exported MP4 in a normal player** (VLC/Movies & TV): the
    cuts, the grade, and the captions are all **baked in** and it plays correctly
    with audio.
24. **Cancel** — start another export of something longer and hit **Cancel**
    mid-way. → It stops promptly, returns to the editor cleanly, and **no partial
    file** is left behind.
25. 🔴 **Feedback** — **Help → Send beta feedback…**, type a line, **Send**. → Your
    mail client opens a new message to `isaiahaniemeka@gmail.com`, subject
    "Cutlass beta feedback", with your text + version/OS footer prefilled. (Send is
    disabled until you type something.)

## D. Stress & polish (if you have time)

26. **Long video** (20 min+) — import it. → Import finishes in seconds (seek-based,
    not a full decode), the timeline filmstrip doesn't lock up, you can cut/scrub.
27. **Multi-track** — add a **V3** and an **A1**, drag clips onto them, remove a
    track. → Stacks composite in the monitor; ✕ removes; V1/V2 can't be removed
    below two.
28. **Never freezes** — during *every* heavy step above (import, transcribe,
    export), the window stays draggable/responsive. → No spinning-cursor lockups.
29. **Undo/redo everywhere** — Ctrl+Z / Ctrl+Y after grades, captions, cuts,
    track changes. → Each reverses cleanly.

---

## Report card

Tell me, per section: **clean**, or the step number(s) that broke and what you saw.
Anything marked 🔴 that fails is a launch-blocker — those are the surfaces a tester
hits that we've never confirmed on your machine. Everything else we triage
together.
