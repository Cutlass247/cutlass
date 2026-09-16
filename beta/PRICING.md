# Cutlass — pricing and how it is delivered

Written for a payment processor's review. Plain, complete, no marketing.

## The product

**Cutlass** is a desktop video editor for Windows 10/11 (64-bit). It is
software the customer downloads and installs on their own computer. The
editing itself runs entirely on their machine — importing, cutting, colour
grading, and exporting the finished video never involve our servers.

Public download and product page: https://cutlass247.github.io/
Source and releases: https://github.com/Cutlass247/cutlass

## What is sold

| | |
|---|---|
| **Cutlass licence** | **$49.00 USD, charged once** |
| Billing | One-time. Not a subscription. Nothing recurring, no renewal, no auto-charge. |
| What the buyer gets | A permanent licence to use Cutlass on one computer, including all current features. No watermark, no time limit, no feature gating. |
| Delivery | Immediate and automatic, on payment. See "How delivery works" below. |

| | |
|---|---|
| **AI minutes top-up** | **$5.00 USD, charged once** |
| Billing | One-time. Optional. A customer can buy it never, once, or repeatedly. |
| What the buyer gets | 300 additional minutes of cloud AI processing, added to their account balance. The minutes do not expire. |
| Delivery | Immediate and automatic, on payment. |

There are no other paid items, no upsells after purchase, and no charges a
customer can incur without choosing to buy.

## Free trial

New users get a **7-day free trial** of the full application. No payment
details are required to start it, so there is nothing to cancel and no
trial-to-paid auto-conversion. When the 7 days end the app stops editing
until a licence is purchased. Trial length is set server-side.

## What the $49 includes, and the one thing metered

Everything that runs on the customer's own computer is unlimited and included
forever: editing, colour grading, export at any resolution, on-device
transcription, and on-device background-music removal.

Two optional features use cloud GPUs we pay for — fast transcription and AI
highlight-finding. Those are included in the $49 with a monthly fair-use
allowance. The app shows the remaining balance in the interface ("N min of AI
left this month"). A customer who exhausts it can either:

- switch to the on-device equivalent, which is **free and unlimited**, or
- buy the optional $5 top-up.

Nobody is ever blocked from editing or exporting by running out of AI minutes,
and nothing is charged automatically when an allowance runs out.

## How delivery works

1. The customer clicks Buy inside the app or on the website.
2. They are sent to a Lemon Squeezy checkout. The app attaches an anonymous
   identifier for their computer to the checkout as custom data. It is a
   one-way hash of a Windows machine ID — not a name, email, or location.
3. On successful payment, Lemon Squeezy calls our server's webhook. The
   server verifies the signature, then records that computer as licensed (or
   adds the top-up minutes).
4. The app re-checks on its next launch or when the window regains focus, and
   unlocks. No licence key to type in, no email to wait for.

Delivery is therefore automatic and typically complete within seconds. There
is no physical shipment.

## Refunds and support

Refunds are honoured on request. Because the software runs on the customer's
own machine, a refund is processed through Lemon Squeezy and the licence is
released server-side.

Support: **Help → Send beta feedback** inside the application, or GitHub
issues at the repository above.

## Data handling

The product is deliberately local-first. The customer's video never leaves
their computer. Two things are sent off-machine:

- **The licence check** sends only the anonymous machine hash described above.
- **Optional cloud AI** sends only the audio (or just the transcript) of the
  clip being processed, which is discarded after processing. This is clearly
  labelled in the interface, and an on-device alternative is always available
  at no cost.

## Status

Windows only. Currently in public beta, distributed free of charge for trial,
with the paid licence being activated now. macOS and Linux are planned.
