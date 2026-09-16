# Cutlass — beta pitch & positioning

Working copy for outreach posts (Reddit/X/Discord), DMs, and the beta
email. Voice: builder-to-creator, zero corporate. Never name-and-shame
competitors in paid placements; organic posts may reference the ToS
controversy factually with a link.

## One-liner

> Cutlass is a video editor where your transcript is your timeline —
> delete the words, and the video cuts itself. On your machine, never
> uploaded, never watermarked, never claimed.

## The wedge audience (in order)

1. **CapCut leavers.** The June 2025 ToS change (perpetual, sublicensable
   license to user content, including private drafts) created a motivated,
   vocal audience actively seeking an exit. They want: simple editing,
   auto-captions, no rights grab. We have all three.
2. **Talking-head creators** (tutorials, podcasts, course makers). Their
   whole edit is "remove the ums and the dead air." That's two clicks in
   Cutlass and a lost evening in a track editor.
3. **Screen recorders and streamers** — OBS captures, gameplay, long
   sessions. Variable frame rate is conformed on import so audio never
   drifts, background music comes off the clip while the voice stays, and
   anything on screen can be blurred with the blur tracking the subject.

## The 90-second demo script

1. Drag in a talking-head clip. *"It's transcribing on my machine —
   nothing uploaded."* Words appear.
2. Click a word → playhead jumps. Delete a sentence → video cuts, gap
   closes.
3. One click: *"✂ 14 filler words."* One click: *"✂ 6 silences."*
   Play the result.
4. Effects tab → click **Cinematic**. *"That's the whole grade — one
   click, previewing live."* (Optional: drop a .cube LUT, or **Generate
   captions** and watch them appear on V2.)
5. Create tab → **✨ Find the best moments.** *"It reads the whole
   recording and pulls the ones that actually land."* Pick one — it comes
   out vertical and captioned, ready to post.
6. Export. Hardware-encoded, done in seconds. *"No watermark. And the
   terms fit on one page: your content is yours."*

## Objection handling

- **"Another subscription?"** Free beta; at launch there will always be a
  one-time-purchase option. Subscriptions only pay for optional cloud.
- **"Is my footage uploaded for AI?"** Your video never leaves the machine
  — no mode uploads it, ever. **Private** transcribes on-device with
  whisper.cpp and needs no network at all. **Fast** sends the audio track
  to cloud GPUs; our server holds it in memory, never writes it to disk,
  and drops it once the words come back. **Find the best moments** sends
  the transcript — in both modes. Editing, grading and export never touch
  a server.
  - Don't say "airplane mode works" unqualified. It is true of Private
    transcription and of everything in the editor, and false the moment
    someone clicks Find the best moments, which needs the network in
    both modes. Overclaiming here is the one that would actually cost us.
- **"Premiere does more."** Yes — and it should, after 30 years. Cutlass
  does the 80% that talking-head creators do daily, 10× faster: transcript
  editing, smart cuts, one-click Looks and LUT grading, chroma key,
  auto-captions, titles, and keyframes. And it does two things Premiere
  can't do at all: edit by transcript natively, and turn a long recording
  into captioned vertical clips on its own.
- **"Mac?"** Windows beta first; the stack (Rust/Tauri/wgpu) is
  cross-platform and macOS is next.

## Beta funnel (v0)

- Landing page (site/index.html) → mailto waitlist for now; swap to a
  real form (Buttondown/Tally) before any public post.
- Give every beta tester the installer + a 3-line quickstart + a direct
  feedback channel (Discord or email).
- Ask each tester for one thing: a clip of their honest first 5 minutes.

## Shipped since v0 (safe to demo/claim)

- One-click Looks + .cube LUT grading (live preview), color controls,
  sharpen/grain/vignette, chroma key, motion presets (punch-in, Ken
  Burns), auto-captions, voice cleanup, transitions, titles, keyframes.
- In-app beta feedback (Help → Send beta feedback).

## Not yet true (don't claim)

- macOS/Linux builds, collaboration (not in this build at all — it is not
  a relay problem any more, the feature is gone), mobile, background
  removal / masks.

  Auto-updates came off this list at 0.1.2: every build from then on
  updates itself, and it is safe to claim.
