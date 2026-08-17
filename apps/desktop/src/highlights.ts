// On-device highlight finder. Scores candidate moments from signals we already
// have — audio energy (waveform) for laughter/excitement, and transcript text
// for hooks, questions, emphasis, and laughter markers — then ranks the best
// short-ready windows. Nothing leaves the machine.

import type { Word } from "./ipc";

export interface Highlight {
  start: number; // source-media seconds
  end: number;
  label: string; // the opening line, used as a hook/title
  score: number; // 0..1
  reasons: string[]; // human-readable why-it-was-picked tags
}

// clip-length targets (seconds)
const TARGET = 30;
const MIN = 12;
const MAX = 60;

const HOOKS = [
  "wait", "actually", "honestly", "the thing is", "here's the", "you won't believe",
  "guess what", "the best part", "the craziest part", "the worst part", "turns out",
  "plot twist", "nobody tells you", "here's why", "the secret",
];
const EMPHASIS = [
  "crazy", "insane", "unbelievable", "incredible", "amazing", "ridiculous", "wild",
  "shocking", "mind-blowing", "never", "best", "worst", "huge", "massive", "perfect",
  "obsessed", "favorite", "hilarious", "genius", "brutal",
];
const REACTION = ["wow", "whoa", "oh my god", "omg", "no way", "holy", "what the", "finally"];

interface Sentence {
  text: string;
  start: number;
  end: number;
}

/// Group words into sentence-ish units on punctuation or a speech gap.
function sentences(words: Word[]): Sentence[] {
  const out: Sentence[] = [];
  let cur: Word[] = [];
  const flush = () => {
    if (!cur.length) return;
    out.push({
      text: cur.map((w) => w.text.trim()).join(" ").replace(/\s+([.,!?])/g, "$1"),
      start: cur[0].start,
      end: cur[cur.length - 1].end,
    });
    cur = [];
  };
  for (let i = 0; i < words.length; i++) {
    cur.push(words[i]);
    const ends = /[.!?]["')\]]?$/.test(words[i].text.trim());
    const gap = i + 1 < words.length ? words[i + 1].start - words[i].end : 999;
    if (ends || gap > 0.6) flush();
  }
  flush();
  return out;
}

/// Mean/peak waveform amplitude across a time window.
function windowEnergy(wf: number[], duration: number, a: number, b: number) {
  if (wf.length === 0 || duration <= 0) return { mean: 0, peak: 0 };
  const n = wf.length;
  const i0 = Math.max(0, Math.floor((a / duration) * n));
  const i1 = Math.min(n, Math.ceil((b / duration) * n));
  let sum = 0;
  let peak = 0;
  let c = 0;
  for (let i = i0; i < i1; i++) {
    sum += wf[i];
    if (wf[i] > peak) peak = wf[i];
    c++;
  }
  return { mean: c ? sum / c : 0, peak };
}

/// Score a window's transcript text for highlight signals.
function scoreText(text: string) {
  const t = ` ${text.toLowerCase()} `;
  let s = 0;
  const reasons = new Set<string>();
  if (/\b(a?ha(ha)+|hah|lol|lmao|rofl)\b/.test(t) || /\[laugh/.test(t)) {
    s += 0.5;
    reasons.add("😂 Laughter");
  }
  const q = (text.match(/\?/g) || []).length;
  if (q) {
    s += Math.min(0.3, q * 0.15);
    reasons.add("❓ Hook question");
  }
  const ex = (text.match(/!/g) || []).length;
  if (ex) s += Math.min(0.2, ex * 0.1);
  if (HOOKS.some((h) => t.includes(h))) {
    s += 0.25;
    reasons.add("🎣 Strong hook");
  }
  const emph = EMPHASIS.filter((w) => t.includes(` ${w}`)).length;
  if (emph) {
    s += Math.min(0.3, emph * 0.12);
    reasons.add("🔥 Punchy language");
  }
  if (REACTION.some((w) => t.includes(w))) {
    s += 0.15;
    reasons.add("😮 Reaction");
  }
  if (/\b\d+\b/.test(t) && /\b(reasons|ways|things|tips|steps|rules|mistakes)\b/.test(t)) {
    s += 0.2;
    reasons.add("🔢 Listicle");
  }
  return { score: Math.min(1, s), reasons };
}

/// Find and rank up to `top` highlight-worthy windows in source-media time.
export function findHighlights(
  words: Word[],
  waveform: number[],
  duration: number,
  top = 5
): Highlight[] {
  const sents = sentences(words);
  if (sents.length === 0) return [];

  const baseline =
    waveform.length > 0 ? waveform.reduce((a, b) => a + b, 0) / waveform.length : 0;

  // Value each sentence on its own — text signals + how loud/energetic it is.
  const sv = sents.map((s) => {
    const { score: textScore, reasons } = scoreText(s.text);
    const { mean, peak } = windowEnergy(waveform, duration, s.start, s.end);
    let energyScore = 0;
    if (baseline > 0) {
      energyScore = Math.max(0, Math.min(1, mean / baseline - 1));
      if (peak > 0.85) energyScore = Math.min(1, energyScore + 0.3);
    }
    if (energyScore > 0.45) reasons.add("🔊 Big reaction");
    return { score: 0.5 * textScore + 0.5 * energyScore, reasons };
  });

  // Anchor a window on each sentence so the strong moment is the *opening* line
  // (the hook), then grow forward toward the target length.
  const cands: Highlight[] = [];
  for (let i = 0; i < sents.length; i++) {
    let k = i;
    let end = sents[i].end;
    while (k + 1 < sents.length && sents[k + 1].end - sents[i].start <= MAX) {
      k++;
      end = sents[k].end;
      if (end - sents[i].start >= TARGET) break;
    }
    const start = sents[i].start;
    if (end - start < MIN && !(i === 0 && sents.length === 1)) continue;

    // dominated by the anchor (which becomes the label), plus a bonus for a
    // second strong beat landing inside the window.
    const reasons = new Set(sv[i].reasons);
    let bestOther = 0;
    for (let j = i + 1; j <= k; j++) {
      bestOther = Math.max(bestOther, sv[j].score);
      for (const r of sv[j].reasons) if (r.startsWith("😂") || r.startsWith("🔊")) reasons.add(r);
    }
    cands.push({
      start,
      end,
      label: sents[i].text.slice(0, 64) + (sents[i].text.length > 64 ? "…" : ""),
      score: sv[i].score + 0.3 * bestOther,
      reasons: [...reasons],
    });
  }

  // greedily take the highest-scoring, non-overlapping windows.
  cands.sort((a, b) => b.score - a.score);
  const picked: Highlight[] = [];
  for (const c of cands) {
    if (picked.length >= top) break;
    const overlaps = picked.some((p) => {
      const o = Math.min(p.end, c.end) - Math.max(p.start, c.start);
      return o > 0.4 * Math.min(p.end - p.start, c.end - c.start);
    });
    if (!overlaps) picked.push(c);
  }
  return picked.sort((a, b) => a.start - b.start);
}
