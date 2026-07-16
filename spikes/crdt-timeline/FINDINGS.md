# Spike A findings — timeline as CRDT

**Verdict: viable.** Run it yourself: `npm install && npm run spike` (Node ≥23).

Two simulated editors ("Ava", "Ben") diverge offline and merge. All
scenarios converge deterministically on both replicas.

## What we proved

| Scenario | Outcome |
|---|---|
| Both move the same clip | Converges to one winner; **losing value is recoverable** via `getConflicts`, so the UI can show "Ben also moved this clip to 0:00.5 — keep yours?" |
| Both add clips to the same track | Both clips survive; derived render order identical on both sides |
| Delete vs. trim of the same clip | Deterministic: **delete wins** in Automerge. Product must surface "your trim landed on a deleted clip" |
| 50 offline edits each side, one merge | Full convergence; 107-change history in a **942-byte** document |

## Design rules this locks in

1. **Clips live in a map keyed by id; playback order is DERIVED from
   `start` time, never stored as CRDT list order.** List-position merges
   produce nonsense for timelines; per-property last-writer-wins is what
   editors actually expect.
2. **Conflicts are UI events, not errors.** Automerge always converges;
   `getConflicts` hands us the losing values so the collaboration UI can
   offer one-click "restore their version."
3. **Delete-vs-edit needs a product answer, not a tech one.** Automerge's
   delete-wins is fine as the merge rule; the trimming editor gets a
   non-blocking notification (their trim is recoverable from history).
4. **History is nearly free.** Every change is retained and the document
   stays tiny (~1KB for 100+ changes) — version history and multiplayer
   undo come from the same log that powers sync.

## Gotchas discovered

- `A.save()` bytes are NOT identical across converged replicas (per-actor
  metadata) — compare heads + content, never bytes.
- JS object key insertion order differs per replica (each lists its own
  adds first). Meaningless in our model, but any serialization/diff code
  must canonicalize key order.

## What the real implementation adds (out of spike scope)

- Move = (track, start) pair updated atomically in one change block.
- Ranged ops (ripple delete, roll edits) as single transactions so they
  merge as a unit.
- Automerge sync protocol over WebSocket (spike used direct `merge`).
- The Rust core would use `automerge-rs` — same format, same semantics.
