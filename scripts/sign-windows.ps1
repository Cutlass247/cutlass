# Authenticode-sign one file with Azure Artifact Signing.
#
# Called by Tauri once per built artifact via bundle.windows.signCommand, so a
# normal `npm run tauri build` produces signed output with nothing to remember.
#
# The credential is read from outside the repository and never printed. It can
# sign software as Isaiah, so it is treated like the licensing key: see
# ~/.cutlass-keys/READ-ME-azure-signing.txt.
#
#   scripts\sign-windows.ps1 <file-to-sign>

param([Parameter(Mandatory = $true)][string]$File)

$ErrorActionPreference = "Stop"

$cred = Join-Path $env:USERPROFILE ".cutlass-keys\azure-signing-sp.json"
if (-not (Test-Path $cred)) {
    # Loud on purpose. A signing step that quietly does nothing produces an
    # unsigned installer that looks exactly like a signed one until a user
    # hits SmartScreen -- the same silent-failure shape as a missing .sig.
    Write-Error "No signing credential at $cred. Cannot sign $File."
    exit 1
}

$sp = Get-Content $cred -Raw | ConvertFrom-Json
$env:AZURE_CLIENT_ID     = $sp.appId
$env:AZURE_TENANT_ID     = $sp.tenant
$env:AZURE_CLIENT_SECRET = $sp.password

$cli = Join-Path $env:USERPROFILE ".cargo\bin\artifact-signing-cli.exe"
if (-not (Test-Path $cli)) {
    Write-Error "artifact-signing-cli not found. Install it: cargo install artifact-signing-cli"
    exit 1
}

& $cli `
    -e "https://eus.codesigning.azure.net" `
    -a "cutlasssigning" `
    -c "cutlass-release" `
    -d "Cutlass" `
    $File

if ($LASTEXITCODE -ne 0) {
    Write-Error "Signing failed for $File (exit $LASTEXITCODE)"
    exit $LASTEXITCODE
}

# Do not trust the exit code alone -- confirm Windows actually accepts it.
$sig = Get-AuthenticodeSignature $File
if ($sig.Status -ne "Valid") {
    Write-Error "Signed $File but Windows reports '$($sig.Status)' - refusing to continue."
    exit 1
}
Write-Host "signed: $(Split-Path $File -Leaf) -> $($sig.SignerCertificate.Subject)"
