# Code signing — the plan

**The problem it solves:** every downloader sees *"Windows protected your PC — unknown publisher."* Plenty of people will not click past that, and the ones who do are the ones least likely to also hand over $49. Signing attaches a verified publisher identity so Windows knows who shipped it.

**Status:** Cutlass 0.1.3 is **unsigned** — confirmed, not assumed:

```
Get-AuthenticodeSignature .\Cutlass_0.1.3_x64-setup.exe
  status : NotSigned
```

Now worth doing, because payments are being switched on. An unknown-publisher warning in front of a paying customer is friction at the worst possible moment.

---

## Two things, often confused

1. **A valid signature** — proves *who* published it. Removes "unknown publisher."
2. **SmartScreen reputation** — earned over downloads and time.

You need the first to start earning the second.

## The options, as of September 2026

| Option | Cost | Where | SmartScreen | Token? |
|---|---|---|---|---|
| **Microsoft Store (MSIX)** | **Free** | Worldwide | **No warnings at all** | No |
| **Azure Artifact Signing** (was Trusted Signing) | ~$9.99/mo | Orgs: US/CA/EU/UK · **Individuals: US + Canada only** | Reputation builds | No |
| **OV certificate** (DigiCert, Sectigo…) | $150–300/yr | Worldwide | Reputation builds | **Yes** (HSM/USB) |
| **EV certificate** | $400+/yr | Worldwide | Reputation builds — **same as OV** | **Yes** |
| Self-signed | Free | — | Blocks installation | — |

Source: [Code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options) (Microsoft Learn, updated 2026-08-29).

## ⚠️ Two corrections to what this document used to say

**EV no longer bypasses SmartScreen.** That behaviour was removed in **2024**. An earlier version of this file recommended EV for "instant trust, day one" — that advice is dead, and following it would mean paying $400+/yr for something identical to a $150 OV cert. If you already hold an EV cert, keep using it; do not buy one for this reason.

**Nothing paid gives instant trust any more.** Azure, OV and EV all start at zero reputation and build it by signing consecutive releases with the *same* identity. So the benefit compounds — and switching identities later resets it. Pick one and stay on it.

## The one option that does remove warnings: the Store

Publishing an **MSIX** through the Microsoft Store is free, worldwide, and Microsoft re-signs the package so users see **no SmartScreen warning at all**. It is the only path to zero warnings on day one.

The catches, for Cutlass specifically:
- Tauri builds NSIS and MSI, not MSIX. Packaging work required.
- Store review, Store policies, and Store's cut on anything sold through it — though Cutlass sells through Lemon Squeezy, not in-app, so that may not apply.
- The bundled ffmpeg is LGPL; Store distribution needs the licence compliance checked, not assumed.

Worth pricing as a *second* channel later. Not a reason to delay signing now.

## Recommendation

**Settled 2026-09-14: Azure Artifact Signing, Basic SKU (~$9.99/mo).** Isaiah is
in the US, so the individual tier applies — no hardware token, no LLC, and it
drops into the existing build. The blocker this document used to name, needing a
legal entity, no longer exists.

Then **sign every release under the same identity**, so reputation accumulates
rather than restarting from zero each version.

If the individual tier ever falls through, the fallback is an **OV certificate**
at $150–300/yr with a USB token or cloud HSM. Not EV — see above.

## Setting up Azure Artifact Signing (Isaiah: US individual — this is the path)

Source: [Quickstart: Set up Artifact Signing](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart)
(Microsoft Learn, updated 2026-09-12).

### Where this stands (2026-09-15)

Set up, awaiting Microsoft's review. None of this is secret.

| | |
|---|---|
| Signing account | `cutlasssigning` |
| Region | East US |
| Endpoint | `https://eus.codesigning.azure.net` |
| Resource group | `cutlass` |
| SKU | Basic (~$9.99/mo) |
| Roles assigned | Identity Verifier · Certificate Profile Signer |
| Identity validation | Individual / Public — **Verified ID completed 2026-09-15**, awaiting Microsoft (1–20 business days) |
| Certificate profile | **not yet created** — blocked until validation completes |

Next, in order: validation reaches **Completed** → create a **Public Trust**
certificate profile bound to it → wire `signCommand` → sign and verify.

### Read this before you start anything

**Your Azure billing account's legal name and address become the certificate,
verbatim.** For individual validation the form is populated from the billing
account and is **read-only** — you cannot correct it there. If the name or
address is wrong, you have to change the billing account and *start a new
identity validation request*, which invalidates work already done.

So, first:

- Billing account **Account Type must be `Individual`** (not Organization).
- The **legal name must match your government photo ID exactly.**
- The address must match your ID, a utility bill, or a bank statement.
- City, state and country from that address are printed on the certificate.
  Your email and street address are not.

Fix those *before* step 4, not after.

### The steps

1. **Azure subscription + Entra tenant.** A personal Microsoft account is
   enough; no company required.
2. **Register the resource provider** — `Microsoft.CodeSigning`:
   `az provider register --namespace Microsoft.CodeSigning`
3. **Create an Artifact Signing account**, Basic SKU (~$9.99/mo, 5,000
   signatures — far more than we will use). Pick a US region and note its
   endpoint; e.g. East US → `https://eus.codesigning.azure.net`.
4. **Assign yourself the `Artifact Signing Identity Verifier` role.** Without
   it the **New identity** button is greyed out with no explanation. This is
   the step people get stuck on.
5. **Create the identity validation** — Individual → Public. Select the
   billing account; the form fills itself from it (see the warning above).
6. **Complete Verified ID on your phone.** A third party (AU10TIX) emails a
   PIN, takes a phone number, then you scan a QR code and photograph your
   **government ID** — passport, driver's licence or state ID. Ends in the
   Microsoft Authenticator app. **Have your phone and ID to hand**; it is a
   live capture, not an upload of an old scan. No flash, flat surface, no
   cropping, both sides as separate images.
7. **Create a certificate profile** of type **Public Trust**, bound to the
   completed identity validation.

**Timing:** the Verified ID part takes minutes. Microsoft quotes **1–20
business days** for public identity validation overall, so start it now rather
than the week of a launch.

## Wiring it into the build

Tauri signs during `tauri build`, in `apps/desktop/src-tauri/tauri.conf.json` under `bundle.windows`:

Azure has no local certificate file to point at, so `certificateThumbprint`
does not apply. Use `bundle.windows.signCommand`, which Tauri runs over each
built artifact with the path substituted for `%1`:

```jsonc
"bundle": {
  "windows": {
    "signCommand": "trusted-signing-cli -e https://eus.codesigning.azure.net -a <ACCOUNT> -c <PROFILE> %1"
  }
}
```

`trusted-signing-cli` is a small Rust tool (`cargo install trusted-signing-cli`)
wrapping the Azure signing API. It authenticates from the environment, so a
local `az login` covers manual builds.

**Check those flags against the tool's current README at setup time.** This is
written from the service documentation, not from a run — it is the one part of
this document nobody has executed, and the rest of the project has been bitten
twice now by exactly that kind of assumption.

If the fallback OV path is ever needed instead, `signtool.exe` is already on
this machine:
`C:\Program Files (x86)\Windows Kits\10\bin\10.0.17763.0\x64\signtool.exe`
— that path takes `certificateThumbprint` plus a `timestampUrl`.

**Sign the installer *and* the exe inside it.** Both are `NotSigned` today, and
a signed installer that drops an unsigned binary still trips warnings later.

**Verify afterwards — do not assume it worked.** A misconfigured signing step is silent:

```powershell
Get-AuthenticodeSignature "target\release\bundle\nsis\Cutlass_<version>_x64-setup.exe"
# status must be Valid, and SignerCertificate.Subject must be you
```

Add that to the release checklist in `RELEASING.md` once a cert exists.

## Note on the updater

Update signing (the minisign key in `~/.cutlass-keys/updater.key`) is **separate and unrelated**. It proves an update came from us; Authenticode proves the installer came from a verified publisher. Both are needed and neither replaces the other. Do not conflate them.

## Bottom line

- **Cheapest real path:** ~$120/yr (Azure), if the geography allows it.
- **Fallback:** $150–300/yr (OV).
- **Do not buy EV.**
- **No option except the Store removes warnings immediately** — signing starts the clock, it doesn't skip it.
