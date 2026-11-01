// How a title is laid out in the frame — the preview's copy of the exporter's
// rule (crates/cutlass-core/src/export.rs `fit_title`). drawtext never wraps,
// so the export breaks lines itself; the preview uses the same table and the
// same rule, so the line breaks you see are the ones that get exported.
// Keep every number here identical to export.rs.

export const TITLE_MAX_W = 0.9;
export const TITLE_BOX_PAD = 14;

/// Advance width of one character as a fraction of the font size, for the
/// bold sans titles are drawn in. Generous on purpose: overestimating wraps a
/// word early, underestimating cuts letters off.
export function glyphEm(c: string): number {
  if (" ilj!|.,:;'’".includes(c)) return 0.28;
  if ("ftrI()".includes(c)) return 0.4;
  if (c === "m") return 0.9;
  if (c === "w") return 0.82;
  if (c === "M" || c === "W") return 0.98;
  if (c >= "A" && c <= "Z") return 0.74;
  return 0.6;
}

export function textEm(s: string): number {
  let em = 0;
  for (const c of s) em += glyphEm(c);
  return em;
}

/// Lines and font size (px) for `text` drawn at `fs` px in a frame `frameW` px
/// wide: the fewest lines that fit, balanced, shrinking the font only when one
/// word alone is wider than the frame.
export function fitTitle(text: string, fs: number, frameW: number): { lines: string[]; fs: number } {
  const words = text.split(/\s+/).filter(Boolean);
  if (!words.length) return { lines: [text], fs };
  const room = Math.max(1, frameW * TITLE_MAX_W - 2 * TITLE_BOX_PAD);
  const longest = Math.max(...words.map(textEm));
  const size = longest * fs > room ? Math.max(8, room / longest) : fs;
  const lay = (limitEm: number): string[] => {
    const lines: string[] = [];
    let cur = "";
    for (const w of words) {
      const next = cur ? `${cur} ${w}` : w;
      if (cur && textEm(next) > limitEm) {
        lines.push(cur);
        cur = w;
      } else cur = next;
    }
    lines.push(cur);
    return lines;
  };
  const limit = room / size;
  const n = lay(limit).length;
  if (n === 1) return { lines: [words.join(" ")], fs: size };
  let lo = longest;
  let hi = limit;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    if (lay(mid).length <= n) hi = mid;
    else lo = mid;
  }
  return { lines: lay(hi), fs: size };
}
