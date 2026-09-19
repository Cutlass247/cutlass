# Going live on Lemon Squeezy

Test mode is a separate environment. Live mode has its own products, its own
variant ids, its own checkout links and its own webhook secret. Nothing copies
across, and nothing in this repo needs rebuilding — going live is entirely a
matter of creating two products and setting five environment variables.

## The thing that silently costs money

The payment webhook grants a purchase to the machine named in the order's
custom data:

```rust
let hwid = v["meta"]["custom_data"]["hwid"].as_str().unwrap_or("").trim();
if order_id.is_empty() || hwid.is_empty() || variant.is_empty() {
    return Ok(StatusCode::OK);
}
```

An order that arrives without one is **dropped, and answered 200**. Lemon
Squeezy records a clean delivery, so nothing retries and nothing alerts. The
card is charged and the customer gets nothing, and the first anyone hears of
it is an email from them.

So a checkout must never be reachable without an hwid. `GET /buy/:what`
enforces this: it forwards to the configured checkout only when given a valid
machine id, and sends everyone else to the download page. That is why the
landing page's button lands on the download rather than a checkout — a
visitor who has not installed yet has no machine to grant a licence to.

Guarded by `a_live_checkout_is_never_reached_without_a_machine_to_grant_to`
(server) and `buying_goes_through_our_own_server_and_names_the_machine`
(desktop). If either starts failing, purchasing is broken; do not ship past it.

## 1. Create the two products

In the dashboard, **switch out of Test mode first.** Anything created in test
is a different object that will not exist live.

Both products are **one-time** payments, not subscriptions. The compliance
review was approved against `beta/PRICING.md`, which states both items are
one-time and nothing recurring — a subscription here contradicts it.

Both are digital products with **no file to deliver**. The purchase is
fulfilled by the webhook granting entitlement to a machine, not by a download.

Both must be **Published**. A draft's buy link 404s, and that link is going
into an environment variable.

### Leave Lemon Squeezy's own "License keys" feature OFF

Cutlass issues its own codes through `/redeem`. If LS also generates one, the
buyer gets a key in their receipt that does nothing in the app, and writes in
to ask why.

### Licence — $49.00, one-time

Copy for the product page lives in this file's sibling, `beta/PRICING.md`,
which is the wording the reviewer approved. Keep the two consistent: no
"everything stays on your machine" claims, because the two cloud AI features
send audio or a transcript to the server. Footage never leaves; that
distinction is the one worth keeping exact.

### AI minutes top-up — $5.00, one-time

300 minutes. The minutes live in the Railway config, not in Lemon Squeezy —
LS only needs the price right.

### Tax category

Choose the category for **downloadable / installed software**, not SaaS.
Cutlass installs and runs locally; it is not a hosted service. As merchant of
record, Lemon Squeezy applies VAT from this field, and it is the only one on
the form with tax consequences.

## 2. Create the live webhook

- URL: `https://cutlass-production.up.railway.app/webhook/lemonsqueezy`
- Event: **`order_created`** — the only one acted on; everything else is
  acknowledged and ignored.
- Copy the signing secret. It is **not** the test-mode secret.

## 3. Set five Railway variables

| Variable | Value | Format |
|---|---|---|
| `CUTLASS_LS_SIGNING_SECRET` | live webhook signing secret | the string as given |
| `CUTLASS_LS_CHECKOUT_LICENSE` | licence buy link | full URL, no query string |
| `CUTLASS_LS_CHECKOUT_CREDITS` | credits buy link | full URL, no query string |
| `CUTLASS_LS_LICENSE_VARIANTS` | licence **numeric** variant id | `2050448`-style; comma-separated if ever more than one |
| `CUTLASS_LS_CREDIT_VARIANTS` | credits variant id **and minutes** | `<id>:300` — the `:300` is required, a bare id is ignored |

Leave `CUTLASS_LS_ALLOW_TEST_MODE` **unset**.

**Either spelling of "licence" works.** `CUTLASS_LS_CHECKOUT_LICENCE` and
`CUTLASS_LS_LICENCE_VARIANTS` are accepted alongside the `LICENSE` forms,
because everything written about this project spells it the British way and
the variables spell it the American way. Setting the wrong one used to be
silent — green deploy, working health check, buying quietly dead. If both are
set, the `LICENSE` form wins; a blank one is ignored in favour of the other. A test order is signed and says
"paid", and once a product has been copied to live mode the two can share a
variant id — so nothing else would tell them apart.

### The variant id is not the checkout UUID

This is the one value the dashboard hides, and the one that fails quietly. The
server reads `data.attributes.first_order_item.variant_id`, a number. The UUID
in the buy link is a different identifier. Put the UUID in
`CUTLASS_LS_LICENSE_VARIANTS` and every real order falls through to "unknown
product — ack, grant nothing", which looks exactly like a working webhook.

Two ways to get it, in order of preference:

1. Open the variant for editing; the numeric id is in the browser's URL bar.
2. Read it out of a real order's payload: **Settings → Webhooks → delivery
   log**. This is literally the field the server reads, so it cannot disagree.

**If you had to use (2), the order is recoverable.** When the variant is
unrecognised the handler returns *before* writing to `webhook_events`, so the
order is not marked processed. Set the variable, then hit **Resend** on that
delivery and it will be granted properly. No refund-and-rebuy needed.

## 4. Verify before trusting it

```bash
curl -s https://cutlass-production.up.railway.app/checkout
```

Both links, not `null`.

```bash
curl -sI "https://cutlass-production.up.railway.app/buy/license" | grep -i location
```

Must still be the download page. **If this shows a checkout, stop** — that is
the unattributed path, and every sale through it is money taken for nothing.

```bash
curl -sI "https://cutlass-production.up.railway.app/buy/license?hwid=$(printf 'a%.0s' {1..64})" | grep -i location
```

Must be the live checkout with `checkout[custom][hwid]=aaa…` on the end.

## 5. Buy it once, for real

Worth the $49 of float. The end-to-end verification on record was done in test
mode, and test mode is precisely the environment that can differ.

Buy from inside a **trial** build — the Creator edition has no Buy button.
Then confirm the licence actually landed:

```bash
curl -s "https://cutlass-production.up.railway.app/usage?hwid=<your machine id>"
```

Then clear the grant and refund yourself:

```bash
curl -s -X POST https://cutlass-production.up.railway.app/admin/reset \
  -H "X-Admin-Token: <token>" -H "Content-Type: application/json" \
  -d '{"hwid":"<your machine id>"}'
```

## What does not need doing

**No app release.** No store URL is compiled into Cutlass. `openCheckout`
opens `<licence-server>/buy/<kind>?hwid=<id>` and the server redirects to
whatever the current checkout is, so the store can move and every installed
copy follows. This was not always true: the checkout links used to be
constants in `ipc.ts`, which would have meant every copy already installed
opening a dead test checkout the day the store went live.

**No landing-page change.** Its button already goes through `/buy/license`.

## Recovery codes

Every licence purchase mints a `CUTLASS-XXXX-XXXX-XXXX` code, returned in the
webhook's response body and stored against the order.

This matters because a webhook grant binds the licence to a machine id derived
from the Windows MachineGuid, and that changes when somebody reinstalls
Windows, swaps a drive or replaces the computer. Without a code, a customer who
bought outright is locked out of it with no way back except emailing you. With
one, they redeem it on the new machine and the licence transfers, releasing the
old one.

Look one up when a customer asks:

```bash
curl -s "https://cutlass-production.up.railway.app/admin/code?order=<order id>" -H "X-Admin-Token: <token>"
```

`?hwid=<machine id>` works too. Purchases made before this existed have no code
on record — mint one with `/admin/mint`.

A licence order that arrives **without** a machine id (a web sale) still mints
a code and grants nothing; the code is the delivery. A **credit** order without
one grants nothing and says so, because credits attach to a machine and have no
code form — refund it, or add the minutes by hand once you know the buyer's id.

## If the website should sell directly

It does not today, by design — see the top of this file. The alternative is to
mint a `CUTLASS-XXXX-XXXX-XXXX` code for an order that arrives with no hwid
and have Lemon Squeezy deliver it in the confirmation email; the buyer then
redeems it in the app's "Enter license key" box, which already works. That is
a change to the webhook plus delivery configured in LS. It has not been built.
