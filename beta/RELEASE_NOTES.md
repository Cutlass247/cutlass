# Cutlass 0.2.5

**Cut sharper. Own everything.**

Windows 10/11 · 64-bit. Free 7-day trial, then **$49 once — yours forever.** No subscription, no watermark.

## New in 0.2.6

- **SVG pictures import.** A logo saved as SVG comes in sharp at any size, styles, gradients and text included.
- **Camera RAW photos import.** Canon, Nikon, Sony, Fujifilm, Olympus, Panasonic, Pentax, Leica and DNG raw files are developed by Windows' own raw decoder, the one the Photos app uses, and turned the way the camera was held.
- **Apple Log footage through a LUT exports cleanly.** The LUT used to be applied after the picture was cut down to 8 bits, which doubled its error and could band smooth skies and walls. It now runs on the footage's full 10 bits. The export was also labelled as wide-gamut BT.2020 when its colours were standard Rec.709, so QuickTime, Safari and other colour-managed players showed it oversaturated. It's now labelled what it is.
- **Clips above V1 export with everything you gave them.** Keyframed movement, scale and rotation now export there, and so do speed, a LUT, temperature and tint, which were left out, and brightness, contrast and blur keyframes, which used to stay at their first value.
- **Keyframes glide in the export, as they do in the player.** Movement, colour, blur and volume keyframes used to export in fifth-of-a-second steps, so a slide came out jerky and a fade-up on a logo stepped. They now change on every frame.
- **Blur boxes follow a tracked face smoothly.** A box moved by the motion tracker used to jump five times a second in the export, so a face moving quickly could show between jumps. It now moves with every frame.
- **Vignette, film grain, sharpen and blur boxes work above V1.** They were left out of the export on V2 and higher. The player also shows every visible clip's vignette and blur boxes now: a logo on V2 used to hide V1's vignette, and only the selected clip's boxes were shown.
- **You hear fades and volume keyframes while playing.** They used to be heard only in the export.
- **Music on an audio track exports with its settings.** Fade in and out, speed, volume keyframes, voice cleanup, audio sync and music removal were all ignored on audio tracks, so a music bed you faded out played to the end at full volume. Only its volume came through before.
- **Dissolves keep each clip's look, speed and sound.** A graded clip flashed ungraded for the length of a dissolve. A sped-up clip slowed to normal speed and jumped back in its footage. Its sound lost its cleanup, sync and music removal.
- **Sped-up clips stay in sync.** After a clip at 2x, the picture of everything that followed ran about two frames behind its sound.
- **Transitions show while playing**, not only when paused or scrubbing.
- **iPhone HDR video looks right.** It showed and exported washed out and grey. Cutlass now makes a standard-range copy in the background when you import it.
- **The player's colours match the export.** Most HD and 4K footage previewed slightly off what the export renders.
- **Keyframes stay on the footage** when you split, trim or retime a clip. The right half of a split lost its animation, and trimming the left edge slid it against the picture.
- **Deleting words from a sped-up clip cuts the right footage.** In a clip at 2x, removing words from the transcript cut the wrong part.
- **A keyframed clip with a fade fades once.** It flickered through black five times a second.
- **A clip turns about its own centre on V1**, as in the player, so a clip both moved and turned lands where you put it.

## New in 0.2.5

- **iPhone photos.** HEIC and HEIF pictures now import, whole and the right way up. An iPhone saves a photo as a grid of tiles with its rotation stored separately, so Cutlass puts the tiles together and turns the picture once, when you import it. Your original file isn't changed.
- **Nearly every other kind of picture.** AVIF, JPEG XL, JPEG 2000, Photoshop (.psd), Targa (.tga), icons (.ico), QOI, DDS, PCX, Netpbm (.ppm, .pgm, .pbm, .pam), SGI, Sun raster, XBM, XPM and XWD join PNG, JPEG (including .jfif and .jpe), WebP, BMP, GIF and TIFF.
- **Portrait phone photos are listed the right way round.** A JPEG taken upright showed and exported upright, but the media bin and the Inspector gave its size as landscape.

## New in 0.2.4

- **Pictures.** PNG, JPEG, WebP, BMP, TIFF and GIF files now import alongside your video. A picture can be on screen for as long as you like: it starts at five seconds, and you drag its edge to change that. Transparency is kept, so a logo or a watermark on V2 shows your footage around it and through any holes, in the player and in the export. An animated GIF comes in as a video. Create makes clips from a video, so add pictures in Studio.
- **Clips above V1 export the way you placed them.** Scale, position, rotation, flips, blur and fades on V2 and higher tracks showed in the player but were left out of the export, so a picture-in-picture or a logo moved into a corner came out filling the frame. They now export as you see them, and the space around a smaller clip shows the track beneath instead of black bars.
- **The Studio player fills its frame.** While you scrubbed or played, the picture could sit at its own small size in the middle of a large player, then jump to full size when you stopped.

## New in 0.2.3

- **Import MP3s and other audio files.** MP3, WAV, M4A, AAC, FLAC, OGG, Opus, WMA, AIFF and more now come in through File → Import and by dragging them onto the window. MP3s couldn't be imported at all before. A sound file goes on an audio track, for music, a voiceover or sound effects under your video, and shows its waveform in the media bin. Create makes clips from a video, so add audio in Studio.
- **More kinds of video.** Camcorder footage (.mts, .m2ts), TV and OBS recordings (.ts), DVD video (.vob, .mpg), phone video (.3gp), and .mxf, .dv, .ogv and .asf join MP4, MOV, MKV, WebM, AVI, WMV and FLV. The Import dialog lists all of them; it used to show only six.
- **WMA and WMV sound plays.** The audio in WMA files, and in WMV videos, decoded to silence. It now plays, draws its waveform and can be transcribed.
- **Sound starts on time when you play from the middle of a clip.** It used to begin slightly late: a few hundredths of a second for most files, and up to a second for FLAC and WavPack.
- **TS, MPG, VOB and FLV show and export the right frame.** These files have no index for jumping to a point in them, so an export could start on a corrupt frame, show nothing in a clip's last second, or run a frame or two early, and the preview of a .ts file could be 1.4 seconds out. When you import one, Cutlass now makes an indexed copy in the background (it takes seconds and loses no quality) and edits that.

## New in 0.2.2

- **Captions fit the frame.** A caption wider than a vertical clip ran off both edges in the export, its first and last letters simply missing. Long captions now break into balanced, centred lines that fit every clip shape, and the player lays them out the same way, so what you see is what you export. Titles in Studio also preview at the size they export; they used to look up to half again too large.
- **Export in Create exports the clip you made.** The Export button and File → Export now give you the vertical, square or wide clip you picked, instead of a letterboxed 16:9 video. You name the file before it's saved, starting from the moment's title, and Cancel really cancels. Create exports always level your voice and match platform loudness.
- **Clicking a clip no longer moves it.** A click could nudge a clip by a hundredth of a second: enough to open a hairline gap after a split, hide the Transition panel and add a step to undo. A clip now moves only when you drag it.
- **Transitions look in the player the way they export**, with the next clip blending in over the end of this one. They're offered only on the lowest video track, the one the export draws them on.
- **No more phantom captions.** The word being spoken was drawn over the player like a burned-in caption, so captions appeared with Captions off and doubled with them on. It's highlighted in the transcript instead.
- **Studio warns before replacing a file** that already exists, and **Escape closes menus and dialogs.** Shortcuts no longer act behind an open dialog; Delete used to remove the selected clip behind the export window.
- **Find the best moments copes with long recordings.** A long video could fail with *"empty response"*, or take long enough that Cutlass gave up and ran the on-device finder under a "no connection" banner. It now waits for a slow answer, and says so plainly if one never comes.
- **Long jobs say how long they'll take.** Transcribing and removing music show the time left, and music removal warns you before it starts on a long clip.
- **Clearer licence messages.** A recovery code that couldn't reach the server is no longer called wrong; the message tells you the code is still good.

## New in 0.2.1

- **Transitions export again.** Any timeline with a cross-dissolve or a dip to black failed to export, on every encoder, after the video library Cutlass renders with changed how it reports frame rates. If an export of yours stopped with *"current rate of 1/0 is invalid"*, this was the cause — try it again on 0.2.1.
- If you installed 0.2.0 before 27 October, this update also brings you the layout fixes listed under 0.2.0 below.

## New in 0.2.0

- **Retiming keeps the footage you were looking at.** Changing a clip's speed used to hold its slot on the timeline and pull in whatever footage filled it at the new rate — so speeding a clip up jumped the picture somewhere else and retimed that instead. A retimed clip now gets shorter or longer, the way it does everywhere else, and keeps the footage it had.
- **Pan now moves the part of the picture that's actually being cropped.** Reframing keeps whichever side of your footage overflows the shape you picked — the sides when a landscape clip goes into a Short, the top and bottom when a phone video goes into a wide frame. The slider only ever moved sideways before, so for vertical footage it did nothing at all, which is the case where the crop discards the most picture.
- **Nothing in the editor overlaps itself, whatever the size of the window.** Slider names in the Inspector — "Cleanup strength" among them — could vanish under their own sliders when the panel was narrow; they now wrap, and in a very narrow Inspector each slider drops onto its own line beneath its name. Widening a side panel or the timeline can no longer squeeze the player until its controls spill over the panels beside it, and during the trial the banner along the top no longer pushes the bottom of the timeline off the screen.

## New in 0.1.3

- **"Enhance audio" now does something.** The switch was reaching the exporter as "off" no matter how it was set, so the levelling, the music ducking and the loudness match were all quietly skipped. Ticking it now levels your voice and matches the loudness platforms expect — expect exports to sound noticeably more even than before.
- **Reframing keeps the framing you chose.** Panning a clip to keep your subject in a vertical crop was being dropped on the way to the export, so it came out centred regardless of where you put it.

## New in 0.1.2

- **Exported audio no longer picks up a faint background hiss.** Audio was being encoded at a bitrate where the encoder ran short of bits and spilled its noise unevenly into the left and right channels — barely visible in a meter, clearly audible on headphones, and worst on the near-mono voice recordings this app is mostly used for. Screen recordings had it twice over, because the conversion they get on import was doing the same thing again. Exports now measure identical to the source.

## New in 0.1.1

- **Drag a video onto the window to import it.** This simply did nothing before — now a file dropped anywhere on Cutlass lands in the bin, including on the player, where the first-run message asks you to drop one.
- **A first run tells you what to do next** instead of telling you what is missing: drop a video, then drag the clip onto a track.
- **Export defaults to 1080p** — matching the preset the dialog opens on — and never above your footage, because exporting 720p at 1080p cannot add detail and only makes the file bigger.
- **A failed export can be tried again or reported**, rather than leaving you with an encoder error and a Close button. Your timeline is untouched either way, and several of the things that stop a render clear on a second attempt.

## Download

**Cutlass_0.2.5_x64-setup.exe** (Windows 10/11, 64-bit) — attached below.

Code-signed: Windows shows the publisher as **Isaiah Aniemeka**. SmartScreen may warn on a new release while its reputation builds — **More info → Run anyway**.

## What Cutlass does

### Find your best moments, automatically

Drop in a long recording and Cutlass reads the whole thing, then pulls out the funniest, most exciting and most pivotal moments — each one turned into a captioned vertical clip, ready to post. Real comprehension, not keyword-guessing.

Choose **Fast** to transcribe on cloud GPUs in seconds, or **Private** to do it entirely on your own machine. With no connection at all, Private does the whole job locally — finding moments included.

### Edit by talking

Delete a sentence in the transcript and the video cuts with it: frame-accurate, rippled shut, undoable. Filler words and dead air are found for you; two clicks and they're gone.

### Made for screen recordings

OBS and other variable-frame-rate captures are evened out on import, so cuts and audio stay in sync — no drift, no fiddling. The clip opens straight away and the conversion happens behind it.

Strip copyrighted music off a clip while keeping your voice, on-device. Blur a face, a screen or a name, and the blur follows the subject as they move.

### Finish it properly

One-click Looks preview live on your footage, or drop in a `.cube` LUT, pull green screen and dial colour by hand. One switch on export levels the volume, ducks music under your voice and matches the loudness every platform expects.

Hardware-accelerated H.264 export, plus ProRes and WebM.

### Two editors, one app

**Create** is the clip maker: footage in, captioned vertical clip out. **Studio** is a full multi-track editor with effects, titles and keyframes. They're independent — experiment in one without touching the other.

## Buy it once

**$49, one time.** No subscription, ever. Every update to the version you own is free. If a big new version ever arrives it'll be an optional paid upgrade, discounted for existing owners, and your copy keeps working exactly as it does today.

Bought on the website? Collect your licence code at **<https://cutlass-production.up.railway.app/claim>** with the email you paid with and the order number on your receipt.

Your video never leaves your machine. The AI features send only your audio or transcript, then discard it — never your footage. Your content is yours; Cutlass claims no licence to anything you make.

## Known limits

- Windows only. macOS and Linux are on the roadmap.
- SmartScreen reputation is still building, and so is the reputation the installer needs with antivirus software.
- 4K exports are slow by nature. Export at 1080p for long videos.
- Film grain, sharpen, and a blur box's size or strength, when keyframed, change in fifth-of-a-second steps rather than gliding. (A box's movement glides.)
- Sharpen shows in the export, not in the player's preview.
- Camera RAW photos need Microsoft's free Raw Image Extension from the Microsoft Store; Cutlass tells you if it's missing. They're developed with Windows' default look, like a camera's own JPEG; for your own edit, develop them in a photo editor first.
- With Apple Log footage and a LUT, the player's preview is a few shades off the export (the export is the accurate one).
- Not supported: OpenEXR and HDR pictures, because without tone mapping they'd look wrong.
- Collaboration isn't available in this build.
- The offline moment finder has no comprehension — it can't tell you *why* a moment works the way the AI can. A good fallback, not a replacement.

## Something wrong?

**Help → Send feedback** inside Cutlass, or open an issue. Please include your **graphics card**, and for anything export-related the clip length and the file size you got.
