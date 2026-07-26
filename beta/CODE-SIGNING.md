# Code signing — the plan

**The problem it solves:** every tester today sees *"Windows protected your PC — unknown publisher."* Many normal people will not click past that. Signing attaches a verified publisher identity to the installer so Windows (and SmartScreen) trust it. It is the single biggest friction-remover before a wide launch.

**For the closed beta:** skip it. The "More info → Run anyway" note in the README/invite is enough for a small group of people who already trust you. Do **not** spend money here yet — wait until real testers confirm the app is worth signing.

---

## How Windows trust actually works

Two separate things, and this trips people up:

1. **A valid signature** — proves *who* published it (identity). Removes "unknown publisher."
2. **SmartScreen reputation** — earned over downloads/time. A brand-new certificate can still show a warning until reputation accrues, **unless** the cert type grants instant reputation.

So the cert type matters a lot.

## The three realistic options (2026)

| Option | ~Cost | SmartScreen | Hardware token? | Catch |
|---|---|---|---|---|
| **Azure Trusted Signing** | ~$10/mo | Trusted (builds fast) | No (cloud) | Needs a **verified business** (historically 3+ yrs; individual tier has been rolling out — verify eligibility) |
| **OV certificate** (Sectigo/DigiCert) | ~$200–400/yr | Warning until reputation builds | **Yes** (USB token / cloud HSM) | Reputation ramp; token complicates automated builds |
| **EV certificate** | ~$300–700/yr | **Instant trust**, day one | **Yes** (token) | Most expensive; token handling |

Notes:
- Since 2023, OV/EV certs can no longer be plain `.pfx` files — the private key must live on a hardware token or cloud HSM (CA/Browser Forum rule). That's why **Azure Trusted Signing** (cloud, no token) is now the sweet spot for most indies.
- **Self-signed certificates do nothing for SmartScreen** — don't bother.

## The entity requirement (read this first)

Code-signing certs — and Azure Trusted Signing's org tier — are issued to a **verified legal entity**, not usually a person. As a solo founder that likely means **registering an LLC** (or checking the individual/sole-proprietor path, which some CAs and Azure's newer individual tier support). This is the real gating step and it takes days–weeks, so start it before you think you need it.

## Recommendation

1. **Now (closed beta):** ship unsigned with the "Run anyway" note. Zero spend.
2. **In parallel:** if you don't have a business entity, start forming one (an LLC) — it's needed for signing anyway, and for taking payment later.
3. **Before any wide/public launch:** get signing in place. In priority order of value-for-money:
   - **Azure Trusted Signing** if you qualify — cheapest, cloud, good SmartScreen behavior.
   - **EV cert** if you want zero warnings on day one and can absorb the cost.
   - **OV cert** as a budget fallback, accepting a short reputation ramp.

## Wiring it into the build (when you have a cert)

Tauri signs the installer during `tauri build`. In `apps/desktop/src-tauri/tauri.conf.json`, under `bundle.windows`:

- **Token/local cert:** set `"certificateThumbprint"` (and `timestampUrl`, e.g. `http://timestamp.digicert.com`). Tauri calls `signtool` with it.
- **Azure Trusted Signing:** use `bundle.windows.signCommand` to invoke Azure's signing tool on the built artifacts, or run a post-build `signtool` step with the Azure dlib.

Then re-run the installer build and verify: right-click the `.exe` → Properties → **Digital Signatures** tab should show your publisher name.

## Bottom line

- **Cost to start:** $0 for the closed beta.
- **Cost before public launch:** ~$120/yr (Azure) to ~$400/yr (OV/EV), plus whatever forming an entity costs.
- **Biggest hidden dependency:** a legal entity. Start that early; everything else is a config change once the cert exists.
