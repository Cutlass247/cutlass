# Cutlass — pricing strategy

Decided pre-launch (during beta) so it's ready to apply. Model is locked; exact feature-line and final number get a sanity check from beta signal before going live.

## The principle

Local features you **own**; cloud features you **rent**. That's the whole model, and it maps to both our cost structure and the ownership pledge (*"a one-time purchase will always be an option; subscriptions only pay for optional cloud"*). Never violate it — it's why subscription-refugees trust us.

## The three tiers

### Free — growth-first, genuinely complete
The goal: a solo creator can make a great talking-head video and **publish it, watermark-free**, without paying. This is our funnel and our CapCut counter-punch. It's affordable to give away because editing runs **on-device** — near-zero marginal cost per free user.

Includes:
- Full **transcript editing** + click-to-cut (the hook — always free)
- **Smart cuts** (filler words + silences)
- **Auto-captions** from the transcript
- Core timeline editing: trim, blade, up to **2 video + 1 audio track**
- Core **Looks** + basic color (brightness/contrast/saturation/temp)
- Transitions, titles/lower-thirds
- **Export: H.264 up to 1080p**, no watermark
- Save/open projects, auto-save

### Pro — $99 one-time (own it forever)
Everything in Free, plus the pro polish, delivery formats, and power tools:
- Full **grading**: all LUTs (.cube), green-screen / chroma key, all stylize effects (grain, sharpen, vignette, hue)
- **Keyframes** (animate any parameter)
- **Unlimited tracks**
- **4K export**
- **ProRes master** + all export formats
- Speed/retime, motion presets
- (Provisional — beta tells us which of these creators expect free vs. pay for.)

**Ownership terms:** you own the current major version forever. Big new versions (v2, v3) are **optional paid upgrades** (~50% off for existing owners). This is how a one-time model stays sustainable without ever forcing a subscription (the Affinity / Sublime model).

### Cloud / Teams — ~$8–12 per user / month (optional)
The only recurring charge, because it's the only thing that costs us servers:
- Hosted **multiplayer collaboration**
- Cloud project **sync + backup**
- **Team seats** / shared workspaces

## Price anchoring (why $99 works)

| Competitor | Price | Model |
|---|---|---|
| Descript (closest rival) | ~$144–288/yr | Subscription + cloud (uploads your footage) |
| Adobe Premiere | ~$276/yr | Subscription |
| Final Cut Pro | $300 | One-time (Mac) |
| DaVinci Resolve Studio | $295 | One-time |
| **Cutlass Pro** | **$99** | **One-time — less than one year of Descript, yours forever** |

## Launch tactics

- **Founder's price: ~$59 lifetime** for the first buyers. **Beta testers get Pro free, for life** — a thank-you and a base of advocates.
- Frame it with honest urgency: *"Founder's price won't last."* No fake countdowns.
- Regional / purchasing-power pricing and an education discount: consider post-launch, not day one.

## Validate during beta (don't finalize blind)

- Ask testers point-blank: **"What would you pay for this?"** and **"Which feature would you happily pay to unlock?"**
- Watch which features they actually reach for — that's where the free/Pro line really belongs. Move a line or two based on signal (e.g., if everyone lives in the LUTs, that's clearly Pro; if captions are what makes them tell friends, keep it free).
- Confirm the $99 anchor feels like a no-brainer, not a wince.

## Implementation (later, not now)

- Sell through a **Merchant-of-Record** (Paddle or Lemon Squeezy), not raw Stripe — they handle global sales tax/VAT so we don't. License-key delivery for Pro unlock.
- Requires the **legal entity** (same one needed for code signing) — start that early.
- Free vs Pro is enforced by a license check gating the Pro features listed above; the app stays fully on-device either way.
