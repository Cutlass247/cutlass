// The Effects-tab catalog. A "Look" is a one-click cinematic grade (a
// bundle of colour params); an "Effect" drops params (or runs an action)
// on the selected clip. Both apply via set_effects (one undo step) and
// preview instantly where CSS can.

export interface EffectPreset {
  name: string;
  desc?: string;
  params?: Record<string, number>;
  action?: "kenburns"; // special handlers that need keyframes, not just fx
}

export interface EffectGroup {
  title: string;
  items: EffectPreset[];
}

// Each Look fully specifies its colour params so re-applying is idempotent
// and "None" clears them back to neutral.
export const LOOKS: EffectPreset[] = [
  { name: "None", desc: "Neutral", params: { brightness: 0, contrast: 1, saturation: 1, temperature: 0, tint: 0, hue: 0, vignette: 0 } },
  { name: "Cinematic", desc: "Teal & orange, gentle vignette", params: { brightness: -0.02, contrast: 1.16, saturation: 1.12, temperature: 18, hue: -8, vignette: 0.35 } },
  { name: "Warm", desc: "Golden-hour glow", params: { brightness: 0.03, contrast: 1.05, saturation: 1.18, temperature: 45, hue: 6, vignette: 0 } },
  { name: "Cool", desc: "Cold, moody blue", params: { brightness: 0, contrast: 1.06, saturation: 1.0, temperature: -45, hue: -12, vignette: 0 } },
  { name: "Vintage", desc: "Faded film, warm cast", params: { brightness: 0.05, contrast: 0.9, saturation: 0.72, temperature: 30, hue: 10, grain: 0.25, vignette: 0.42 } },
  { name: "Noir", desc: "High-contrast black & white", params: { brightness: -0.03, contrast: 1.42, saturation: 0, temperature: 0, hue: 0, vignette: 0.5 } },
  { name: "B&W", desc: "Clean monochrome", params: { brightness: 0, contrast: 1.12, saturation: 0, temperature: 0, hue: 0, vignette: 0 } },
  { name: "Vivid", desc: "Punchy, saturated", params: { brightness: 0.02, contrast: 1.2, saturation: 1.4, temperature: 0, hue: 0, vignette: 0 } },
];

export const EFFECT_GROUPS: EffectGroup[] = [
  {
    title: "Color",
    items: [
      { name: "Warmer", desc: "Warm white balance", params: { temperature: 40 } },
      { name: "Cooler", desc: "Cool white balance", params: { temperature: -40 } },
    ],
  },
  {
    title: "Stylize",
    items: [
      { name: "Blur", desc: "Gaussian blur", params: { blur: 6 } },
      { name: "Sharpen", desc: "Crisp up detail", params: { sharpen: 0.6 } },
      { name: "Film grain", desc: "Analog texture", params: { grain: 0.4 } },
      { name: "Vignette", desc: "Darken the edges", params: { vignette: 0.45 } },
      { name: "Hue shift", desc: "Rotate the colours", params: { hue: 40 } },
    ],
  },
  {
    title: "Motion",
    items: [
      { name: "Punch-in", desc: "Zoom in 20%", params: { scale: 1.2 } },
      { name: "Ken Burns", desc: "Slow push-in", action: "kenburns" },
    ],
  },
  {
    title: "Mirror",
    items: [
      { name: "Mirror", desc: "Flip horizontally", params: { flip_h: 1 } },
      { name: "Flip", desc: "Flip vertically", params: { flip_v: 1 } },
    ],
  },
  {
    title: "Key",
    items: [
      { name: "Green screen", desc: "Key out green (use on V2+)", params: { chroma: 1, chroma_sim: 0.3 } },
    ],
  },
  {
    title: "Audio",
    items: [{ name: "Voice cleanup", desc: "Reduce hiss & room noise", params: { denoise: 1 } }],
  },
];
