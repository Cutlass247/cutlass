# Cutlass — pricing

What ships today, and the reasoning behind it. Everything here describes the
product as sold; anything still undecided says so.

## The model

**Free 7-day trial, then $49 once. Everything included.**

No free tier, no Pro/Free feature gate, no watermark, no subscription. The
trial is the full app — the same build a buyer runs — and it ends by asking for
$49, not by taking features away.

The trial is server-backed (an anonymous machine ID, never your footage), so it
can't be reset by reinstalling. Your licence keeps working offline for **30
days** after its last check; the trial gets 3.

### The principle it comes from

Local features you **own**; cloud features you **rent**. That maps to the cost
structure — editing runs on-device, at near-zero marginal cost per user — and to
the ownership pledge: *a one-time purchase will always be an option;
subscriptions only ever pay for optional cloud.* Never violate it. It is why
subscription-refugees trust us, and it is the whole argument against Descript.

### Ownership terms

You own the current major version forever. Big new versions (v2, v3) would be
**optional paid upgrades** (~50% off for existing owners) — the Affinity /
Sublime model, which is how a one-time price stays sustainable without ever
forcing a subscription.

> Not yet said publicly anywhere. The README and the landing page promise
> "$49 once — yours forever", which is true of the version someone buys but
> does not mention paid major upgrades. Decide whether to state it before v2
> exists, not after — announcing it later reads as a change of terms even when
> it wasn't one.

### AI usage

Finding moments and cloud transcription are included, with generous fair use
for everyday editing. On-device transcription is always unlimited and free,
and with no connection the moment finder runs on-device too. Cloud AI is the
only part with a real marginal cost, and it is metered per machine rather than
sold as a tier.

## Why $49

| Competitor | Price | Model |
|---|---|---|
| Descript (closest rival) | ~$144–288/yr | Subscription + cloud (uploads your footage) |
| Adobe Premiere | ~$276/yr | Subscription |
| Final Cut Pro | $300 | One-time (Mac) |
| DaVinci Resolve Studio | $295 | One-time |
| **Cutlass** | **$49** | **One-time — a third of a year of Descript, yours forever** |

$49 is below the line where people deliberate. "Cheaper than four months of
Descript, and you keep it" is a sentence a creator can repeat to a friend
without doing arithmetic.

## Still undecided

- **Founder's price.** The original plan was ~$59 for first buyers against a
  $99 list. That is dead: the beta already sells at $49, so a $59 "discount"
  would cost more than not having one. Either there is no launch discount, or
  $49 *is* the founder's price and list rises afterwards. Pick one before the
  store goes live, because the second option needs saying up front to be
  honest.
- **Regional / purchasing-power pricing and an education discount.**
  Post-launch, not day one.
- **Cloud / Teams**, if collaboration is ever finished: ~$8–12 per user/month
  for hosted collaboration, cloud project sync and backup, and team seats. The
  only recurring charge we would ever add, because it is the only thing that
  costs us servers. **Collaboration is not in the build today** — this is a
  future option, not an unshipped promise, and must not be sold as one.

## What beta should tell us

- Ask testers point-blank: **"What would you pay for this?"** — and listen for
  a wince at $49, which would mean the anchor is wrong rather than the pitch.
- Watch which features they actually reach for. That no longer decides a
  free/paid line, but it decides what the landing page leads with.
- **Beta testers get it free, for life.** A thank-you, and a base of advocates
  who were there first.

## Selling it

- **Merchant of Record: Lemon Squeezy**, not raw Stripe — they handle global
  sales tax and VAT so we don't. Currently in **test mode, awaiting their
  review**; going live means copying the products to Live Mode, pointing the
  webhook at the licence server, and setting the five `CUTLASS_LS_*` variables
  on Railway.
- Licence keys are minted and checked by the licence server on Railway. The app
  stays fully on-device either way — the server settles entitlement, never
  touches footage.
- **No legal entity needed to sell.** The old plan treated forming one as a
  prerequisite. It isn't for either half: code signing goes through Azure's
  individual tier (US/Canada, no LLC, no hardware token — see
  `beta/CODE-SIGNING.md`), and Lemon Squeezy acts as Merchant of Record for
  individuals too. Whether to form one later is a separate question, and not
  one that blocks taking money.

---

<sub>Superseded 2026-09-16: this document previously described a three-tier
Free / Pro / Cloud model with a feature gate — a free tier capped at 1080p,
with grading, keyframes, unlimited tracks and 4K behind a $99 "Pro" unlock.
None of it shipped. It is recorded here so the change is visible rather than
silently rewritten.</sub>
