/**
 * Spike A — Can a video timeline be a CRDT?
 *
 * Simulates two editors ("Ava" and "Ben") working on the same project
 * offline, then merging. The bet: Automerge gives us convergence,
 * conflict detection, and history for free, and timeline-specific merge
 * semantics are sane if we model the timeline correctly.
 *
 * KEY DESIGN CHOICE under test: clips live in a MAP keyed by id, with
 * playback order DERIVED from each clip's `start` time — we do NOT
 * encode timeline order as CRDT list order. List-position merges produce
 * nonsense for timelines (a clip "between" two others has no meaning);
 * start-time merges produce last-writer-wins per property, which is
 * exactly what editors expect.
 */
import * as A from "@automerge/automerge";

interface Clip {
  name: string;
  track: string; // "V1", "V2", "A1", ...
  start: number; // timeline position, ms
  srcIn: number; // source in-point, ms
  srcOut: number; // source out-point, ms
}

interface Project {
  name: string;
  clips: { [id: string]: Clip };
}

let passed = 0;
let failed = 0;
function check(label: string, cond: boolean, detail = "") {
  if (cond) {
    passed++;
    console.log(`  ✅ ${label}`);
  } else {
    failed++;
    console.log(`  ❌ ${label} ${detail}`);
  }
}

/** Render order = clips sorted by (track, start). Derived, never stored. */
function renderOrder(doc: A.Doc<Project>): string[] {
  return Object.entries(doc.clips)
    .sort(([, a], [, b]) => a.track.localeCompare(b.track) || a.start - b.start)
    .map(([id]) => id);
}

// ---------------------------------------------------------------------------
// Setup: one shared project, then two clients fork it (simulates two
// machines that synced once and then went offline).
// ---------------------------------------------------------------------------
let base = A.from<Project>({
  name: "Trailer v1",
  clips: {},
});
base = A.change(base, (d) => {
  d.clips["intro"] = { name: "intro.mp4", track: "V1", start: 0, srcIn: 0, srcOut: 4000 };
  d.clips["interview"] = { name: "interview.mp4", track: "V1", start: 4000, srcIn: 1000, srcOut: 9000 };
  d.clips["broll"] = { name: "drone.mp4", track: "V2", start: 2000, srcIn: 0, srcOut: 3000 };
});

let ava = A.clone(base);
let ben = A.clone(base);

// ---------------------------------------------------------------------------
// Scenario 1 — both editors move the SAME clip while offline.
// Expectation: both replicas converge to the SAME position (deterministic
// winner), and the conflict is detectable so the UI can surface it.
// ---------------------------------------------------------------------------
console.log("\nScenario 1: concurrent move of the same clip");
ava = A.change(ava, (d) => {
  d.clips["broll"].start = 5000; // Ava slides b-roll late
});
ben = A.change(ben, (d) => {
  d.clips["broll"].start = 500; // Ben slides it early
});
let mergedAva = A.merge(A.clone(ava), ben);
let mergedBen = A.merge(A.clone(ben), ava);

check(
  "replicas converge to identical position",
  mergedAva.clips["broll"].start === mergedBen.clips["broll"].start,
  `(ava-side=${mergedAva.clips["broll"].start}, ben-side=${mergedBen.clips["broll"].start})`
);
const conflicts = A.getConflicts(mergedAva.clips["broll"], "start");
check(
  "conflict is detectable for the UI to surface",
  conflicts !== undefined && Object.keys(conflicts ?? {}).length === 2,
  `(conflicts=${JSON.stringify(conflicts)})`
);
console.log(
  `  → winner: start=${mergedAva.clips["broll"].start}, losing value recoverable: ${JSON.stringify(conflicts)}`
);

// ---------------------------------------------------------------------------
// Scenario 2 — editors add DIFFERENT clips to the same track while offline.
// Expectation: both clips survive the merge; derived order is sensible.
// ---------------------------------------------------------------------------
console.log("\nScenario 2: concurrent adds on the same track");
ava = A.merge(ava, ben); // sync up before next divergence
ben = A.merge(ben, ava);
ava = A.change(ava, (d) => {
  d.clips["title"] = { name: "title.png", track: "V2", start: 0, srcIn: 0, srcOut: 2000 };
});
ben = A.change(ben, (d) => {
  d.clips["outro"] = { name: "outro.mp4", track: "V1", start: 9000, srcIn: 0, srcOut: 3000 };
});
ava = A.merge(ava, ben);
ben = A.merge(ben, ava);

check("both new clips present after merge", "title" in ava.clips && "outro" in ava.clips);
check(
  "derived render order identical on both replicas",
  JSON.stringify(renderOrder(ava)) === JSON.stringify(renderOrder(ben))
);
console.log(`  → order: ${renderOrder(ava).join(" · ")}`);

// ---------------------------------------------------------------------------
// Scenario 3 — Ava DELETES a clip while Ben TRIMS it.
// Expectation: deterministic converged outcome (Automerge: delete wins).
// Product decision: surface "your trim landed on a deleted clip" in UI.
// ---------------------------------------------------------------------------
console.log("\nScenario 3: delete vs. trim of the same clip");
ava = A.change(ava, (d) => {
  delete d.clips["interview"];
});
ben = A.change(ben, (d) => {
  d.clips["interview"].srcOut = 6000; // Ben tightens the interview
});
mergedAva = A.merge(A.clone(ava), ben);
mergedBen = A.merge(A.clone(ben), ava);

check(
  "replicas agree on survivor set",
  ("interview" in mergedAva.clips) === ("interview" in mergedBen.clips)
);
console.log(
  `  → outcome: clip ${"interview" in mergedAva.clips ? "SURVIVES (trim wins)" : "DELETED (delete wins)"}`
);

// ---------------------------------------------------------------------------
// Scenario 4 — heavy offline divergence, then one merge.
// Expectation: byte-identical documents on both sides; history intact.
// ---------------------------------------------------------------------------
console.log("\nScenario 4: heavy offline divergence (50 edits each side)");
ava = A.merge(ava, ben);
ben = A.merge(ben, ava);
for (let i = 0; i < 50; i++) {
  ava = A.change(ava, (d) => {
    d.clips["broll"].start = 5000 + i * 10;
  });
  ben = A.change(ben, (d) => {
    d.clips["title"].srcOut = 2000 + i * 5;
  });
}
ava = A.merge(ava, ben);
ben = A.merge(ben, ava);

// Convergence criterion: identical heads + identical content. Content is
// compared with sorted keys — JS map insertion order differs per replica
// (each lists its own adds first) and carries no meaning in our model.
const canonical = (v: unknown): string =>
  JSON.stringify(v, (_k, val) =>
    val && typeof val === "object" && !Array.isArray(val)
      ? Object.fromEntries(Object.entries(val).sort(([a], [b]) => a.localeCompare(b)))
      : val
  );
check(
  "replicas converge (same heads, same content)",
  JSON.stringify(A.getHeads(ava)) === JSON.stringify(A.getHeads(ben)) &&
    canonical(ava) === canonical(ben)
);
const history = A.getHistory(ava).length;
check("full edit history preserved (undo/version-history for free)", history > 100, `(changes=${history})`);
console.log(`  → ${history} changes in history, doc size on disk: ${A.save(ava).byteLength} bytes`);

// ---------------------------------------------------------------------------
console.log(`\n${"─".repeat(60)}`);
console.log(`Spike result: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
console.log("VERDICT: timeline-as-CRDT is viable. See FINDINGS.md.");
