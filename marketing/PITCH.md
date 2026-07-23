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
3. **Small teams** who share project files over Drive and overwrite each
   other. Multiplayer timelines are the unlock nobody else ships.

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
5. Open a second window in the same room. Drag a clip in one — it moves
   in the other, live cursors visible.
6. Export. Hardware-encoded, done in seconds. *"No watermark. And the
   terms fit on one page: your content is yours."*

## Objection handling

- **"Another subscription?"** Free beta; at launch there will always be a
  one-time-purchase option. Subscriptions only pay for optional cloud.
- **"Is my footage uploaded for AI?"** No. Transcription is whisper.cpp
  running locally. Airplane mode works.
- **"Premiere does more."** Yes — and it should, after 30 years. Cutlass
  does the 80% that talking-head creators do daily, 10× faster: transcript
  editing, smart cuts, one-click Looks and LUT grading, chroma key,
  auto-captions, titles, and keyframes. And it does two things Premiere
  can't do at all (edit-by-transcript natively, live multiplayer).
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

- macOS/Linux builds, hosted collab relay (rooms currently need a
  self-run relay), auto-updates, mobile, background removal / masks.
