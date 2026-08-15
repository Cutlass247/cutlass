import { ReactNode, useCallback, useEffect, useState } from "react";
import { licenseStatus, licenseRedeem, openUrl, LicenseInfo } from "../ipc";

// Where to send people who want to buy. Wire this to real checkout once
// pricing lands; for now it points at the project page.
const PURCHASE_URL = "https://github.com/Cutlass247/cutlass";

/// Wraps the whole app: verifies entitlement on launch, shows a trial
/// countdown while active, and blocks the editor behind a paywall once the
/// trial ends (or a reconnect screen when the server can't be reached).
export function LicenseGate({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<LicenseInfo | null>(null);
  const [checking, setChecking] = useState(true);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setInfo(await licenseStatus());
    } finally {
      setChecking(false);
    }
  }, []);
  useEffect(() => {
    check();
  }, [check]);

  if (checking && !info) {
    return (
      <div className="lic-screen">
        <div className="lic-splash">
          <Blade />
          <div className="lic-splash-text">Checking your license…</div>
        </div>
      </div>
    );
  }

  const lic = info!;

  if (lic.active) {
    return (
      <>
        {lic.status === "trial" && <TrialBanner info={lic} onUpgraded={setInfo} />}
        {children}
      </>
    );
  }

  // Not active → block the app.
  return <LockScreen info={lic} onRecheck={check} onUpgraded={setInfo} />;
}

function Blade() {
  return (
    <svg width="46" height="46" viewBox="0 0 256 256" aria-hidden>
      <defs>
        <linearGradient id="lgb" x1="0" y1="1" x2="1" y2="0">
          <stop offset="0" stopColor="#c8941f" />
          <stop offset=".5" stopColor="#f6d06c" />
          <stop offset="1" stopColor="#c8941f" />
        </linearGradient>
      </defs>
      <polygon points="93,173 93,189 189,133 153.6,112.4" fill="#d9dde3" />
      <polygon points="83,67 83,163 143.6,102.4" fill="#fff" />
      <line x1="58" y1="198" x2="198" y2="58" stroke="url(#lgb)" strokeWidth="12" strokeLinecap="round" />
    </svg>
  );
}

function TrialBanner({ info, onUpgraded }: { info: LicenseInfo; onUpgraded: (i: LicenseInfo) => void }) {
  const [open, setOpen] = useState(false);
  const days = info.days_left ?? 0;
  const urgent = days <= 2;
  return (
    <>
      <div className={`lic-banner${urgent ? " urgent" : ""}`}>
        <span className="lic-banner-msg">
          {days > 0
            ? `${days} day${days === 1 ? "" : "s"} left in your free trial`
            : "Last day of your free trial"}
        </span>
        <button className="lic-banner-btn" onClick={() => setOpen(true)}>
          Enter license key
        </button>
        <button className="lic-banner-buy" onClick={() => openUrl(PURCHASE_URL)}>
          Buy Cutlass
        </button>
      </div>
      {open && (
        <div className="modal-overlay" onPointerDown={() => setOpen(false)}>
          <div className="modal lic-modal" onPointerDown={(e) => e.stopPropagation()}>
            <div className="modal-title">Activate Cutlass</div>
            <RedeemPanel
              machineId={info.machine_id}
              onUpgraded={(i) => {
                onUpgraded(i);
                setOpen(false);
              }}
            />
            <div className="modal-actions">
              <button className="ghost-btn" onClick={() => setOpen(false)}>
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

function LockScreen({
  info,
  onRecheck,
  onUpgraded,
}: {
  info: LicenseInfo;
  onRecheck: () => void;
  onUpgraded: (i: LicenseInfo) => void;
}) {
  const offline = info.needs_online || info.status === "offline";
  return (
    <div className="lic-screen">
      <div className="lic-card">
        <Blade />
        <h1 className="lic-title">
          {offline ? "Connect to continue" : "Your free trial has ended"}
        </h1>
        <p className="lic-msg">{info.message}</p>

        {offline ? (
          <button className="lic-primary" onClick={onRecheck}>
            Try again
          </button>
        ) : (
          <>
            <a
              className="lic-primary"
              href={PURCHASE_URL}
              onClick={(e) => {
                e.preventDefault();
                openUrl(PURCHASE_URL);
              }}
            >
              Buy a license
            </a>
            <div className="lic-or">or enter a license key</div>
            <RedeemPanel machineId={info.machine_id} onUpgraded={onUpgraded} />
            <button className="lic-link" onClick={onRecheck}>
              Re-check license
            </button>
          </>
        )}

        <div className="lic-machine" title="Your machine ID (share with support for help)">
          Machine ID: <code>{info.machine_id}</code>
        </div>
      </div>
    </div>
  );
}

function RedeemPanel({
  machineId,
  onUpgraded,
}: {
  machineId: string;
  onUpgraded: (i: LicenseInfo) => void;
}) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const submit = async () => {
    if (!code.trim() || busy) return;
    setBusy(true);
    setErr(null);
    try {
      const res = await licenseRedeem(code.trim());
      if (res.active) onUpgraded(res);
      else setErr(res.message);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="lic-redeem">
      <div className="lic-redeem-row">
        <input
          className="lic-input"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          placeholder="CUTLASS-XXXX-XXXX-XXXX"
          spellCheck={false}
          autoCapitalize="characters"
        />
        <button className="lic-primary small" onClick={submit} disabled={busy || !code.trim()}>
          {busy ? "Activating…" : "Activate"}
        </button>
      </div>
      {err && <div className="lic-err">{err}</div>}
      <div className="lic-machine subtle">
        This machine: <code>{machineId}</code>
      </div>
    </div>
  );
}
