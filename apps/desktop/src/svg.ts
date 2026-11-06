/// SVG pictures, drawn by the window: ffmpeg can't read SVG, and the browser
/// engine draws it as faithfully as anything can, styles, gradients and text
/// included. Each is drawn once as it's imported, into a PNG the backend keeps
/// (`store_drawn_picture`) and everything else reads.

/// How big an SVG is drawn on its long side: 4K's. It's drawn from shapes,
/// so nothing is lost at any size, and the picture is fitted to the frame
/// wherever it's shown anyway.
export const SVG_LONG_SIDE = 3840;

/// A length as SVG writes one ("120", "120px", "3in", "50mm") in CSS pixels;
/// NaN for none, or a percentage, which means nothing without a page.
function cssPx(v: string | null): number {
  if (!v) return NaN;
  const m = /^\s*([0-9.]+(?:e[-+]?\d+)?)\s*(px|in|cm|mm|pt|pc)?\s*$/i.exec(v);
  if (!m) return NaN;
  const per: Record<string, number> = { px: 1, in: 96, cm: 96 / 2.54, mm: 96 / 25.4, pt: 96 / 72, pc: 16 };
  return parseFloat(m[1]) * per[(m[2] || "px").toLowerCase()];
}

/// Draw `text` (an SVG document) as a PNG `longSide` pixels on its long side,
/// transparency kept. Throws, with a reason a person can act on, for a file
/// that isn't an SVG or can't be drawn.
export async function drawSvg(text: string, longSide = SVG_LONG_SIDE): Promise<Blob> {
  const doc = new DOMParser().parseFromString(text, "image/svg+xml");
  const svg = doc.documentElement;
  if (doc.getElementsByTagName("parsererror").length || svg.nodeName.toLowerCase() !== "svg") {
    throw new Error("this file isn't a readable SVG picture");
  }
  // its own size: width and height, else its viewBox's
  const vb = (svg.getAttribute("viewBox") || "").trim().split(/[\s,]+/).map(Number);
  const hasVb = vb.length === 4 && vb[2] > 0 && vb[3] > 0;
  let w = cssPx(svg.getAttribute("width"));
  let h = cssPx(svg.getAttribute("height"));
  if (!(w > 0) && !(h > 0)) {
    [w, h] = hasVb ? [vb[2], vb[3]] : [300, 150]; // the size a browser gives one with neither
  } else if (!(w > 0)) {
    w = hasVb ? (h * vb[2]) / vb[3] : h;
  } else if (!(h > 0)) {
    h = hasVb ? (w * vb[3]) / vb[2] : w;
  }
  // Drawn bigger, it has to scale: without a viewBox its drawing is in
  // pixels of its own size, so it gets one saying so.
  if (!hasVb) svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  const scale = longSide / Math.max(w, h);
  const W = Math.max(1, Math.round(w * scale));
  const H = Math.max(1, Math.round(h * scale));
  svg.setAttribute("width", String(W));
  svg.setAttribute("height", String(H));

  const url = URL.createObjectURL(
    new Blob([new XMLSerializer().serializeToString(svg)], { type: "image/svg+xml" })
  );
  try {
    // As a picture element, an SVG runs no script and fetches nothing.
    const img = new Image();
    img.src = url;
    await img.decode().catch(() => {
      throw new Error("this SVG couldn't be drawn");
    });
    const canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("couldn't get a canvas to draw this SVG on");
    ctx.drawImage(img, 0, 0, W, H);
    const png = await new Promise<Blob | null>((done) => {
      try {
        canvas.toBlob(done, "image/png");
      } catch {
        done(null); // an SVG holding HTML (foreignObject) can't be read back
      }
    });
    if (!png) throw new Error("this SVG holds content that can't be drawn as a picture");
    return png;
  } finally {
    URL.revokeObjectURL(url);
  }
}

/// `blob`'s bytes as base64, for the backend.
export async function base64Of(blob: Blob): Promise<string> {
  const url = await new Promise<string>((done, fail) => {
    const r = new FileReader();
    r.onload = () => done(String(r.result));
    r.onerror = () => fail(r.error);
    r.readAsDataURL(blob);
  });
  return url.slice(url.indexOf(",") + 1);
}
