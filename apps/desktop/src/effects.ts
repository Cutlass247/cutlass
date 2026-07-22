// The Effects-tab catalog. A "Look" is a one-click cinematic grade (a
// bundle of colour params); an "Effect" drops a single stylize param at a
// sensible default that you then tune in the Inspector. Both apply to the
// selected clip via set_effects (one undo step) and preview instantly.

export interface EffectPreset {
  name: string;
  desc?: string;
  params: Record<string, number>;
}

// Each Look fully specifies its colour params so re-applying is idempotent
// and "None" clears them back to neutral.
export const LOOKS: EffectPreset[] = [
  { name: "None", desc: "Neutral", params: { brightness: 0, contrast: 1, saturation: 1, hue: 0, vignette: 0 } },
  { name: "Cinematic", desc: "Teal & orange, gentle vignette", params: { brightness: -0.02, contrast: 1.16, saturation: 1.12, hue: -8, vignette: 0.35 } },
  { name: "Warm", desc: "Golden-hour glow", params: { brightness: 0.03, contrast: 1.05, saturation: 1.18, hue: 12, vignette: 0 } },
  { name: "Cool", desc: "Cold, moody blue", params: { brightness: 0, contrast: 1.06, saturation: 1.0, hue: -20, vignette: 0 } },
  { name: "Vintage", desc: "Faded film, warm cast", params: { brightness: 0.05, contrast: 0.9, saturation: 0.72, hue: 14, vignette: 0.42 } },
  { name: "Noir", desc: "High-contrast black & white", params: { brightness: -0.03, contrast: 1.42, saturation: 0, hue: 0, vignette: 0.5 } },
  { name: "B&W", desc: "Clean monochrome", params: { brightness: 0, contrast: 1.12, saturation: 0, hue: 0, vignette: 0 } },
  { name: "Vivid", desc: "Punchy, saturated", params: { brightness: 0.02, contrast: 1.2, saturation: 1.4, hue: 0, vignette: 0 } },
];

export const EFFECTS: EffectPreset[] = [
  { name: "Blur", desc: "Gaussian blur", params: { blur: 6 } },
  { name: "Vignette", desc: "Darken the edges", params: { vignette: 0.45 } },
  { name: "Hue shift", desc: "Rotate the colours", params: { hue: 40 } },
  { name: "Mirror", desc: "Flip horizontally", params: { flip_h: 1 } },
  { name: "Flip", desc: "Flip vertically", params: { flip_v: 1 } },
];
