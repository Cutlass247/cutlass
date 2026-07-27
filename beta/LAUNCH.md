# Closed beta — launch playbook

Everything here is a **you** action (accounts, uploads, posting). The material is ready; you pull the triggers.

Goal: get the installer into the hands of **10–20 testers on varied hardware** (Intel, NVIDIA, AND AMD — the export bugs we just fixed only showed on specific GPUs), gather feedback, fix, then widen.

---

## 1. Publish the download (GitHub Releases)

Your **source code stays private** — this public repo holds only the installer and a README.

1. On GitHub, create a **new public repository** named `cutlass` (description: *"Cutlass — cut sharper, own everything. Windows beta."*). Don't add the source.
2. Add `beta/README.md` (from this repo) as that repo's **README.md**. The download buttons in it point at `releases/latest`, so they'll work automatically once step 4 is done.
3. Go to **Releases → Draft a new release**.
   - Tag: `v0.1.0-beta` · Title: `Cutlass 0.1.0 — Windows beta`
   - Body: paste `beta/RELEASE_NOTES.md`.
   - Check **"Set as a pre-release."**
4. **Attach the installer**: drag in
   `D:\Video Editing Idea\target\release\bundle\nsis\Cutlass_0.1.0_x64-setup.exe`
   Publish.

The public download URL is then `github.com/<you>/cutlass/releases/latest`.

> Before you send the link to anyone: download it yourself from the release page on a different machine if you can, install, and export one clip. That's the exact path a tester walks.

## 2. Recruit testers

Closed beta = **personal invites**, not mass posts. Aim for people who make talking-head video (YouTubers, podcasters, course creators) and, ideally, a spread of PC hardware.

**Direct message / email (warm — someone you know):**

> Hey [name] — I built a video editor and I'd love your eyes on it before I launch. It's called Cutlass: you edit by deleting words from the transcript, kill filler words in one click, and grade with one click. Runs fully on your PC, nothing uploaded, no watermark. Windows only for now. Want the beta? Takes 5 min to try — download's here: [link]. If you hit anything weird, there's a "Send feedback" button right in the app. Honest reactions are exactly what I need.

**Community post (where you're already a member — a Discord/subreddit for creators/editors):**

> **Free Windows video editor beta — edit by transcript, grade in one click, nothing uploaded**
> I've been building Cutlass: your transcript is your timeline (delete words → video cuts), one-click filler/silence removal, one-click cinematic Looks. Fully on-device — your footage never leaves your machine, no watermark, no account. Looking for a handful of beta testers on Windows, especially different GPUs (Intel/NVIDIA/AMD). Download + what-to-expect: [link]. Feedback button is built in. Brutal honesty welcome.

**What to ask each tester for:** their first 5 minutes (where it clicked / where it didn't), and — quietly useful — **what GPU they have**, since that's where our gnarliest bugs lived.

## 3. Handle the SmartScreen question up front

Testers *will* ask about the "Windows protected your PC" warning. Get ahead of it in the invite or a pinned note: it's because the beta isn't code-signed yet (signing costs money + takes time; it's on the list), not because anything's wrong — **More info → Run anyway.** Code signing is the biggest single thing that would smooth this; worth doing before any wide/public launch.

## 4. Close the loop

- Watch **Help → Send beta feedback** emails.
- Log every report in **`feedback-tracker.csv`** (open in Excel, or import into Google Sheets via *File → Import → Upload*). Columns: Date, Tester, Source, GPU, OS, Area, Severity, Report, Status, Fixed in, Notes. Delete the two EXAMPLE rows once you start.
  - **Source:** Reddit / In-app / DM / Email
  - **Area:** Import / Transcribe / Edit / Effects / Export / Save-Open / UI / Perf / Feature / Other
  - **Severity:** Blocker / Major / Minor / Nice-to-have
  - **Status:** New / Investigating / Fixing / Fixed / Won't fix / Need info
  - **Always fill GPU** — it's the single most useful field (our worst bugs were GPU-specific). Sort by GPU to spot patterns like "3 AMD users hit the same thing."
- Ship fixes, cut a new `v0.1.x-beta` release (same steps, new tag), tell testers.
- When a batch of testers gets through import → edit → grade → export cleanly on their own hardware, you're ready to widen (public landing page, Reddit/X/Product Hunt — the drafts in `marketing/PITCH.md`).

---

### Not yet done (fine for closed beta, needed before wide launch)
- **Code signing** (removes the SmartScreen warning) — biggest trust win.
- **Public landing page** hosted (site/index.html is ready to deploy).
- **Real waitlist form** (swap the mailto) for collecting emails at scale.
- **macOS build.**
