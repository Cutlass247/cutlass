# Cutlass 0.1.1 — the reliability release

**Cut sharper. Own everything.**

The video editor you don't have to learn. Drop in a long video and **AI finds your best moments** — captioned, vertical, ready to post — or edit by deleting words, and the video cuts itself.

> **Windows 10/11 · 64-bit.** Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

---

## 🆕 What's new in 0.1.1

**🎚️ Enhance audio.** One switch in the export dialog levels out the volume, keeps music underneath the voice instead of fighting it, and matches the loudness platforms expect (−14 LUFS). Viewers forgive bad video; they never forgive bad audio.

**⚡ Long exports actually finish.** A gap between two clips too short to hold a single frame could deadlock the render — ffmpeg stayed alive and kept reporting progress, so it looked slow rather than stuck, and a long timeline would sit at the same percentage forever. Fixed, along with the memory blow-up that made hour-long timelines crawl: peak memory is now flat no matter how many cuts you have.

**🛡️ Your project survives a crash.** Auto-save used to rewrite the project in place, so anything interrupting a save — a crash, a power cut, an external drive dropping out — could leave it unreadable, and a damaged project is a total loss. Saves are now atomic, every save keeps a backup beside it, and a damaged file opens from that backup instead of failing.

**📁 Projects have a home.** New projects go to **Documents → Cutlass Projects**, exports default to your **Videos** folder, and both remember wherever you actually put things.

**🔑 Your licence follows you.** Renaming your PC no longer affects it, and if you move to a new machine — new laptop, fresh Windows, swapped parts — enter your purchase code again and the licence moves with you.

**Also:** ProRes and WebM export correctly from long timelines · colour grading is ~1.6× faster · the export dialog shows elapsed time and an estimate, so a slow render doesn't look frozen · a missing or damaged clip now names the file and says why, immediately, instead of failing half a minute later in ffmpeg's words · titles containing `%` render correctly.

---

## ✨ What's in this build

### 🤖 Find your best moments — automatically
- **✨ Find the best moments.** Drop in a long recording and Cutlass reads the whole thing, then pulls out the funniest, most exciting, and most pivotal moments — and turns each into a captioned vertical clip. Real comprehension, not keyword-guessing.
- **Fast, or fully private.** *Fast* transcribes on cloud GPUs in seconds, even for hour-long videos. *Private* runs entirely on-device — nothing leaves your machine, unlimited and free.
- **Captions, your call.** One toggle burns captions onto every clip, timed to the speech — off by default, never automatic.

### 📲 Make clips for social
- **Vertical, square, or wide.** Reframe to 9:16, 1:1, or 16:9. **Fill** crops edge-to-edge with no black bars, or drop in a **blurred background**; pan to keep your subject framed.
- **Captions from your words.** Click a word to jump there; delete words and the video cuts with them.

### ✂️ Edit fast
- **Edit by talking.** Delete a sentence in the transcript and the video cuts with it — frame-accurate, rippled shut, undoable.
- **Kill the ums in one click.** Filler words and dead-air silences are found automatically. Two clicks and they're gone.
- **Remove background music.** Strip copyrighted music off a clip while keeping your voice — **on-device** AI vocal separation, one click. Made for gameplay, streams, and screen recordings.
- **Made for screen recordings.** OBS and other variable-frame-rate captures are conformed on import, so cuts and audio stay perfectly in sync.
- **Blur out anything.** Blur, pixelate, or black-box a face, screen, or name — with **motion tracking** so it follows the subject.
- **A timeline that obeys.** In Studio, marquee-select clips, move or delete them together, right-click for quick actions, and Ctrl+A to grab everything.

### 🎨 Grade & finish
- **Grade in one click.** Live-preview Looks (Cinematic, Warm, Noir, and more), drop in a `.cube` LUT, pull green screen, punch in, or dial color by hand — and **grade several clips at once**.
- **Fix out-of-sync audio.** Slip a clip's audio earlier or later to line it up with the picture.
- Transitions, titles and lower-thirds, keyframe any effect, speed/retime, and **live multiplayer** timelines in Studio.
- **Fast, native export.** Hardware-accelerated H.264, plus ProRes and WebM.

### 🎛️ Two independent editors, one app
**Create** is the clip maker: footage in, captioned vertical clip out. **Studio** is a full multi-track editor with effects, titles, and keyframes. They're **independent workspaces** — experiment in one without touching the other.

---

## 📥 Install

1. Download **`Cutlass-0.1.0-trial-x64-setup.exe`** below.
2. Run it. Windows SmartScreen will likely say *"Windows protected your PC"* — this beta isn't code-signed yet, not because anything's wrong. Click **More info → Run anyway**.
3. Launch **Cutlass** from the Start menu. On first run you'll start your 7-day free trial (needs internet once to begin).

## 🔒 Privacy

- **Your video never leaves your machine.** The AI features send only your **audio** (or just the transcript) to transcribe and analyze, then discard it — never your footage.
- Prefer zero cloud? **Private** mode does it all on-device.
- The trial checks a license on first launch using an **anonymous machine ID only** — never your footage.

## ⚠️ Known limitations (it's an early beta)

- **Windows 10/11, 64-bit only.** macOS and Linux are on the roadmap.
- **Not code-signed yet** — expect the SmartScreen prompt above.
- **Fast transcription uses the cloud** (audio only, then discarded); switch to **Private** for 100% on-device.
- **Export defaults to your source resolution** — upscaling 1080p to 4K just makes it softer and bigger; the app warns if you go higher.
- **H.264** works everywhere; ProRes and WebM are available; **H.265 is hidden for now** (depends on specific GPU support).
- Expect rough edges — and please tell us about them.

## 🐛 Tell us what breaks

- In the app: **Help → Send beta feedback** (prefilled with version + system info).
- Most useful thing you can send: **a short clip of your honest first five minutes** — where it clicked, where it didn't.

---

<sub>Cutlass · Windows · Your content is yours.</sub>
