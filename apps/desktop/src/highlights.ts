// On-device highlight finder. No AI — it scores candidate clips from signals we
// already have (audio energy + transcript text) and ranks self-contained,
// short-ready moments. Not comprehension, but it hunts for the same shape a
// good short has: a strong hook, a story turn or emotional beat, and a payoff.
// Nothing leaves the machine.

import type { Word } from "./ipc";

export interface Highlight {
  start: number; // source-media seconds
  end: number;
  label: string; // the opening line (the hook), used as a title
  score: number; // relative, higher = stronger
  reasons: string[]; // human-readable why-it-was-picked tags
}

// clip-length targets (seconds)
const TARGET = 28;
const MIN = 14;
const MAX = 55;

// ── signal vocabularies ──────────────────────────────────────────────
// Curiosity/hook openers — great as the FIRST line of a clip.
const CURIOSITY = [
  "here's the", "here's why", "here's what", "here's how", "the reason", "the truth",
  "the secret", "the trick", "the best part", "the worst part", "the craziest", "the problem with",
  "did you know", "let me tell you", "what if", "nobody tells you", "nobody talks about",
  "everyone thinks", "you won't believe", "i'll never forget", "the one thing", "this changed",
];
// Story pivots / turns.
const PIVOT = [
  "but ", "however", "turns out", "suddenly", "the problem", "here's the thing", "plot twist",
  "except", "until", "the moment", "little did", "out of nowhere", "next thing",
];
// Emotional intensity / stakes.
const EMOTION = [
  "crazy", "insane", "unbelievable", "incredible", "amazing", "ridiculous", "wild", "shocking",
  "hilarious", "terrifying", "obsessed", "genius", "brutal", "perfect", "furious", "devastated",
  "love", "hate", "scared", "excited", "best", "worst", "favorite", "never again",
];
// Payoff / conclusions.
const PAYOFF = [
  "that's why", "the point is", "the lesson", "the takeaway", "in the end", "the moral",
  "which means", "and that's how", "so that's", "long story short", "bottom line",
];
const REACTION = ["wow", "whoa", "oh my god", "omg", "no way", "holy", "what the", "finally", "let's go"];
// Opening words that lean on prior context — a clip starting here feels mid-thought.
const REFERENTIAL = new Set([
  "it", "that", "this", "these", "those", "so", "and", "but", "because", "which", "they",
  "them", "he", "she", "then", "also", "plus", "anyway", "yeah", "okay", "um", "uh", "like",
]);

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

const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")}`;
const hits = (t: string, list: string[]) => list.filter((w) => t.includes(w)).length;

/// Per-sentence signal analysis (text only).
interface Sig {
  curiosity: number;
  pivot: number;
  emotion: number;
  reaction: number;
  payoff: number;
  laugh: boolean;
  question: boolean;
  specific: number;
  referentialStart: boolean;
}
function analyze(text: string): Sig {
  const t = ` ${text.toLowerCase()} `;
  const first = text.trim().toLowerCase().split(/\s+/)[0]?.replace(/[^a-z]/g, "") ?? "";
  return {
    curiosity: hits(t, CURIOSITY),
    pivot: hits(t, PIVOT),
    emotion: hits(t, EMOTION),
    reaction: REACTION.some((w) => t.includes(w)) ? 1 : 0,
    payoff: hits(t, PAYOFF),
    laugh: /\b(a?ha(ha)+|hah|lol|lmao|rofl)\b/.test(t) || /\[laugh/.test(t),
    question: text.includes("?"),
    specific:
      /\b\d/.test(t) || t.includes("$") || t.includes("percent") || t.includes(" million") || t.includes(" thousand")
        ? 1
        : 0,
    referentialStart: REFERENTIAL.has(first),
  };
}

/// Greedily keep the highest-scoring windows that don't substantially overlap.
function topNonOverlapping(cands: Highlight[], top: number): Highlight[] {
  cands.sort((a, b) => b.score - a.score);
  const picked: Highlight[] = [];
  for (const c of cands) {
    if (picked.length >= top) break;
    const overlaps = picked.some((p) => {
      const o = Math.min(p.end, c.end) - Math.max(p.start, c.start);
      return o > 0.35 * Math.min(p.end - p.start, c.end - c.start);
    });
    if (!overlaps) picked.push(c);
  }
  return picked.sort((a, b) => a.start - b.start);
}

/// Instant, transcript-free fallback: rank windows by audio energy alone.
function fromAudio(waveform: number[], duration: number, top: number): Highlight[] {
  if (waveform.length === 0 || duration <= 0) return [];
  const baseline = waveform.reduce((a, b) => a + b, 0) / waveform.length;
  if (baseline <= 0) return [];
  const cands: Highlight[] = [];
  for (let start = 0; start + MIN <= duration; start += TARGET / 2) {
    const end = Math.min(duration, start + TARGET);
    const { mean, peak } = windowEnergy(waveform, duration, start, end);
    cands.push({
      start,
      end,
      label: `Loud moment · ${mmss(start)}`,
      score: Math.max(0, mean / baseline - 1) + (peak > 0.85 ? 0.4 : 0),
      reasons: [peak > 0.85 ? "🔊 Big reaction" : "🔊 Louder moment"],
    });
  }
  return topNonOverlapping(cands, top);
}

/// Find and rank up to `top` highlight-worthy clips in source-media time.
export function findHighlights(
  words: Word[],
  waveform: number[],
  duration: number,
  top = 5
): Highlight[] {
  const sents = sentences(words);
  if (sents.length < 2) return fromAudio(waveform, duration, top);

  const baseline = waveform.length > 0 ? waveform.reduce((a, b) => a + b, 0) / waveform.length : 0;
  const sig = sents.map((s) => analyze(s.text));
  const energy = sents.map((s) => {
    const { mean, peak } = windowEnergy(waveform, duration, s.start, s.end);
    if (baseline <= 0) return 0;
    return Math.max(0, Math.min(1, mean / baseline - 1)) + (peak > 0.85 ? 0.3 : 0);
  });

  // pick the most compelling line in [i..k] to use as the title.
  const hookScore = (j: number) =>
    sig[j].curiosity * 2 +
    (sig[j].question ? 1.2 : 0) +
    sig[j].emotion * 0.8 +
    (sig[j].laugh ? 2 : 0) +
    sig[j].reaction * 0.6 +
    energy[j] * 1.2;

  const cands: Highlight[] = [];
  for (let i = 0; i < sents.length; i++) {
    // grow toward the target length, but stop at a natural pause (topic
    // boundary) once we have enough, so distinct moments don't merge.
    let k = i;
    while (k + 1 < sents.length) {
      if (sents[k + 1].end - sents[i].start > MAX) break;
      const curSpan = sents[k].end - sents[i].start;
      const nextGap = sents[k + 1].start - sents[k].end;
      if (curSpan >= MIN && nextGap > 0.8) break; // natural break — end here
      k++;
      if (sents[k].end - sents[i].start >= TARGET) break;
    }
    const start = sents[i].start;
    const end = sents[k].end;
    if (end - start < MIN && !(i === 0 && sents.length <= 2)) continue;

    let titleIdx = i;
    let bestHook = -1;
    for (let j = i; j <= k; j++) {
      const h = hookScore(j);
      if (h > bestHook) {
        bestHook = h;
        titleIdx = j;
      }
    }

    const reasons = new Set<string>();

    // 1) OPENING — a short lives or dies on its first line.
    let opener = 0;
    const o = sig[i];
    if (o.question) {
      opener += 0.35;
      reasons.add("❓ Opens on a question");
    }
    if (o.curiosity) {
      opener += 0.4;
      reasons.add("🎣 Strong hook");
    }
    if (o.emotion) opener += 0.15;
    if (o.referentialStart && !o.question && !o.curiosity) {
      opener -= 0.3; // starts mid-thought, needs prior context
    }

    // 2) BODY — the beats inside the clip.
    let body = 0;
    let hasPivot = false;
    let hasPayoff = false;
    let bestBeat = 0;
    for (let j = i; j <= k; j++) {
      const s = sig[j];
      body += 0.12 * s.emotion + 0.1 * s.reaction + 0.08 * s.specific + energy[j] * 0.5;
      if (s.laugh) {
        body += 0.45;
        reasons.add("😂 Laughter");
      }
      if (s.pivot) hasPivot = true;
      if (s.payoff && j > i) hasPayoff = true;
      bestBeat = Math.max(bestBeat, s.emotion * 0.3 + energy[j]);
      if (energy[j] > 0.55) reasons.add("🔊 Big reaction");
    }
    if (hasPivot) reasons.add("↪️ Story turn");
    if (hasPayoff) reasons.add("✅ Has a payoff");

    // 3) ARC bonus — hook + turn/beat + payoff is a complete little story.
    let arc = 0;
    if (opener > 0.2 && (hasPivot || bestBeat > 0.4) && hasPayoff) {
      arc = 0.4;
      reasons.add("🎬 Complete story");
    }

    // 4) DENSITY — reward clips packed with signal, not one spike in dead air.
    const dur = end - start;
    const density = Math.min(0.3, (reasons.size / dur) * 6);

    const score = opener + Math.min(0.6, body) + arc + density;
    if (score <= 0.05) continue; // skip flat filler

    const title = sents[titleIdx].text;
    cands.push({
      start,
      end,
      label: title.slice(0, 70) + (title.length > 70 ? "…" : ""),
      score,
      reasons: [...reasons].slice(0, 4),
    });
  }

  if (cands.length === 0) return fromAudio(waveform, duration, top);
  return topNonOverlapping(cands, top);
}
