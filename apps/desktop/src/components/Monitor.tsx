import { ReactNode, useEffect, useRef, useState } from "react";
import { CubeLut } from "../ipc";
import { formatTC, IconButton } from "./ui";

// Cache the compiled WebGL program + textures per GL context.
type LutGl = {
  prog: WebGLProgram;
  uFrame: WebGLUniformLocation | null;
  uLut: WebGLUniformLocation | null;
  uSize: WebGLUniformLocation | null;
  frameTex: WebGLTexture;
  lutTex: WebGLTexture;
};
const glCache = new WeakMap<WebGL2RenderingContext, LutGl>();

function lutBundle(gl: WebGL2RenderingContext): LutGl {
  const cached = glCache.get(gl);
  if (cached) return cached;
  const vs = `#version 300 es
in vec2 aPos; out vec2 vUv;
void main(){ vUv = aPos*0.5+0.5; gl_Position = vec4(aPos,0.0,1.0); }`;
  const fs = `#version 300 es
precision highp float; precision highp sampler3D;
uniform sampler2D uFrame; uniform sampler3D uLut; uniform float uSize;
in vec2 vUv; out vec4 frag;
void main(){
  vec3 c = clamp(texture(uFrame, vUv).rgb, 0.0, 1.0);
  vec3 lc = (c*(uSize-1.0)+0.5)/uSize;
  frag = vec4(texture(uLut, lc).rgb, 1.0);
}`;
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    return s;
  };
  const prog = gl.createProgram()!;
  gl.attachShader(prog, compile(gl.VERTEX_SHADER, vs));
  gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, fs));
  gl.bindAttribLocation(prog, 0, "aPos");
  gl.linkProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  const b: LutGl = {
    prog,
    uFrame: gl.getUniformLocation(prog, "uFrame"),
    uLut: gl.getUniformLocation(prog, "uLut"),
    uSize: gl.getUniformLocation(prog, "uSize"),
    frameTex: gl.createTexture()!,
    lutTex: gl.createTexture()!,
  };
  glCache.set(gl, b);
  return b;
}

function renderLut(gl: WebGL2RenderingContext, img: HTMLImageElement, lut: CubeLut) {
  const b = lutBundle(gl);
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  gl.useProgram(b.prog);
  // source frame (unit 0), flipped so it isn't upside-down
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, b.frameTex);
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
  gl.uniform1i(b.uFrame, 0);
  // LUT as an RGB8 3D texture (unit 1)
  const n = lut.size;
  const rgb = new Uint8Array(n * n * n * 3);
  for (let i = 0; i < rgb.length; i++) rgb[i] = Math.round(Math.min(1, Math.max(0, lut.data[i])) * 255);
  gl.activeTexture(gl.TEXTURE1);
  gl.bindTexture(gl.TEXTURE_3D, b.lutTex);
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_R, gl.CLAMP_TO_EDGE);
  gl.texImage3D(gl.TEXTURE_3D, 0, gl.RGB8, n, n, n, 0, gl.RGB, gl.UNSIGNED_BYTE, rgb);
  gl.uniform1i(b.uLut, 1);
  gl.uniform1f(b.uSize, n);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
}

/// A monitor layer graded by a .cube 3D LUT on the GPU (WebGL2).
function LutLayer(p: { src: string; lut: CubeLut; style?: React.CSSProperties }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const gl = canvas.getContext("webgl2");
    if (!gl) return;
    const img = new Image();
    img.onload = () => {
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      renderLut(gl, img, p.lut);
    };
    img.src = p.src;
  }, [p.src, p.lut]);
  return <canvas ref={ref} style={p.style} />;
}

/// A monitor layer with the green chroma-keyed to transparent on a canvas,
/// so lower layers show through. Approximate preview; export is exact.
function KeyedLayer(p: { src: string; sim: number; style?: React.CSSProperties }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const img = new Image();
    img.onload = () => {
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      ctx.drawImage(img, 0, 0);
      const data = ctx.getImageData(0, 0, canvas.width, canvas.height);
      const d = data.data;
      const t = 1.1 + (0.8 - Math.min(0.8, p.sim)); // higher sim = looser key
      for (let i = 0; i < d.length; i += 4) {
        const r = d[i];
        const g = d[i + 1];
        const b = d[i + 2];
        if (g > 70 && g > r * t && g > b * t) d[i + 3] = 0;
      }
      ctx.putImageData(data, 0, 0);
    };
    img.src = p.src;
  }, [p.src, p.sim]);
  return <canvas ref={ref} style={p.style} />;
}

const clampN = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const hexColor = (n: number) => "#" + Math.max(0, Math.min(0xffffff, Math.round(n))).toString(16).padStart(6, "0");

export type Censor = { style: number; x: number; y: number; w: number; h: number; str: number; color: number };

/// Pixelate preview: sample the region of the current frame into a tiny
/// offscreen canvas and blow it back up with smoothing off (a real mosaic).
/// The export does the same with ffmpeg's neighbor scale.
function PixelFill(p: { src: string | null; x: number; y: number; w: number; h: number; str: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || !p.src) return;
    const img = new Image();
    img.onload = () => {
      const iw = img.naturalWidth, ih = img.naturalHeight;
      const sx = (p.x - p.w / 2) * iw, sy = (p.y - p.h / 2) * ih;
      const sw = Math.max(1, p.w * iw), sh = Math.max(1, p.h * ih);
      const block = 4 + p.str * 36;
      const dw = Math.max(1, Math.round(sw / block));
      const dh = Math.max(1, Math.round(sh / block));
      canvas.width = Math.round(sw);
      canvas.height = Math.round(sh);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      const off = document.createElement("canvas");
      off.width = dw;
      off.height = dh;
      const octx = off.getContext("2d");
      if (!octx) return;
      octx.drawImage(img, sx, sy, sw, sh, 0, 0, dw, dh);
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(off, 0, 0, dw, dh, 0, 0, canvas.width, canvas.height);
    };
    img.src = p.src;
  }, [p.src, p.x, p.y, p.w, p.h, p.str]);
  return <canvas ref={ref} className="censor-fill" />;
}

export type CensorItem = Censor & { slot: number };

/// One draggable/resizable censor box inside the shared stage. Coords are
/// normalised to the stage (which matches the displayed video's aspect box),
/// so they map straight to the export's fractional geometry.
function CensorBoxEl(p: {
  censor: CensorItem;
  stageRef: React.RefObject<HTMLDivElement | null>;
  frameSrc: string | null;
  onCensor: (partial: Partial<{ x: number; y: number; w: number; h: number }>, commit: boolean) => void;
}) {
  const { slot, style, x, y, w, h, str, color } = p.censor;

  const drag = (e: React.PointerEvent, mode: "move" | "resize") => {
    e.preventDefault();
    e.stopPropagation();
    const stage = p.stageRef.current;
    if (!stage) return;
    const rect = stage.getBoundingClientRect();
    const startX = e.clientX, startY = e.clientY;
    const s = { x, y, w, h };
    const calc = (ev: PointerEvent) => {
      const dx = (ev.clientX - startX) / rect.width;
      const dy = (ev.clientY - startY) / rect.height;
      if (mode === "move") {
        return { x: clampN(s.x + dx, s.w / 2, 1 - s.w / 2), y: clampN(s.y + dy, s.h / 2, 1 - s.h / 2) };
      }
      return { w: clampN(s.w + dx * 2, 0.03, 1), h: clampN(s.h + dy * 2, 0.03, 1) };
    };
    const onMove = (ev: PointerEvent) => p.onCensor(calc(ev), false);
    const onUp = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      p.onCensor(calc(ev), true);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  const boxStyle: React.CSSProperties = {
    left: `${(x - w / 2) * 100}%`,
    top: `${(y - h / 2) * 100}%`,
    width: `${w * 100}%`,
    height: `${h * 100}%`,
  };
  const blurPx = 4 + str * 22;
  return (
    <div className={`censor-box style-${style}`} style={boxStyle} onPointerDown={(e) => drag(e, "move")} title="Drag to move · corner to resize">
      <span className="censor-num">{slot + 1}</span>
      {style === 1 && (
        <div className="censor-fill" style={{ backdropFilter: `blur(${blurPx}px)`, WebkitBackdropFilter: `blur(${blurPx}px)` }} />
      )}
      {style === 2 && <PixelFill src={p.frameSrc} x={x} y={y} w={w} h={h} str={str} />}
      {style === 3 && <div className="censor-fill solid" style={{ background: hexColor(color) }} />}
      <div className="censor-handle" onPointerDown={(e) => drag(e, "resize")} />
    </div>
  );
}

/// All active censor boxes for the current clip, in one aspect-matched stage.
function CensorLayer(p: {
  censors: CensorItem[];
  resolution: Resolution;
  frameSrc: string | null;
  onCensor: (slot: number, partial: Partial<{ x: number; y: number; w: number; h: number }>, commit: boolean) => void;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  return (
    <div className="censor-stage" ref={stageRef} style={{ aspectRatio: `${p.resolution.w} / ${p.resolution.h}` }}>
      {p.censors.map((c) => (
        <CensorBoxEl
          key={c.slot}
          censor={c}
          stageRef={stageRef}
          frameSrc={p.frameSrc}
          onCensor={(partial, commit) => p.onCensor(c.slot, partial, commit)}
        />
      ))}
    </div>
  );
}

export type Resolution = { label: string; w: number; h: number };
export const RESOLUTIONS: Resolution[] = [
  { label: "720p", w: 1280, h: 720 },
  { label: "1080p", w: 1920, h: 1080 },
  { label: "4K", w: 3840, h: 2160 },
];

export function Monitor(p: {
  layers: {
    key: string;
    src: string;
    style?: React.CSSProperties;
    chroma?: boolean;
    chromaSim?: number;
    lut?: CubeLut | null;
  }[];
  vignette?: number;
  grain?: number;
  censors?: CensorItem[];
  onCensor?: (slot: number, partial: Partial<{ x: number; y: number; w: number; h: number }>, commit: boolean) => void;
  /// When set (Create mode), preview the footage reframed to this output shape.
  format?: { w: number; h: number; reframe: "fill" | "blur"; rx: number } | null;
  titleOverlay?: ReactNode;
  caption: string | null;
  playhead: number;
  playing: boolean;
  canPlay: boolean;
  resolution: Resolution;
  onResolution: (r: Resolution) => void;
  onTogglePlay: () => void;
  onSeek: (t: number) => void;
  contentEnd: number;
}) {
  const [safe, setSafe] = useState(false);
  const frameRef = useRef<HTMLDivElement>(null);

  // Track the frame's pixel size so the Create preview can fit the chosen
  // output shape exactly — max-width/max-height alone break the aspect ratio
  // when the frame is much taller or wider than the target format.
  const [frameSize, setFrameSize] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = frameRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) =>
      setFrameSize({ w: e.contentRect.width, h: e.contentRect.height })
    );
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Largest box of the format's aspect that fits inside the frame (with margin).
  const stageBox = (() => {
    if (!p.format || frameSize.w === 0) return null;
    const a = p.format.w / p.format.h;
    const fw = frameSize.w * 0.94;
    const fh = frameSize.h * 0.92;
    return fw / fh > a ? { w: fh * a, h: fh } : { w: fw, h: fw / a };
  })();

  return (
    <section className="monitor">
      <div className="monitor-frame" ref={frameRef}>
        {p.layers.length ? (
          p.format ? (
            <div
              className="create-stage"
              style={
                stageBox
                  ? { width: `${stageBox.w}px`, height: `${stageBox.h}px` }
                  : { aspectRatio: `${p.format.w} / ${p.format.h}`, maxWidth: "94%", maxHeight: "92%" }
              }
            >
              {p.format.reframe === "blur" && (
                <img
                  className="cs-bg"
                  src={p.layers[p.layers.length - 1].src}
                  alt=""
                  draggable={false}
                />
              )}
              <img
                className="cs-fg"
                src={p.layers[p.layers.length - 1].src}
                alt=""
                draggable={false}
                style={{
                  objectFit: p.format.reframe === "fill" ? "cover" : "contain",
                  objectPosition: `${(p.format.rx ?? 0.5) * 100}% 50%`,
                }}
              />
            </div>
          ) : (
            <div className="monitor-layers">
              {p.layers.map((l) =>
                l.lut ? (
                  <LutLayer key={l.key} src={l.src} lut={l.lut} style={l.style} />
                ) : l.chroma ? (
                  <KeyedLayer key={l.key} src={l.src} sim={l.chromaSim ?? 0.3} style={l.style} />
                ) : (
                  <img key={l.key} src={l.src} alt="" draggable={false} style={l.style} />
                )
              )}
            </div>
          )
        ) : (
          <div className="monitor-empty">No clip under the playhead</div>
        )}
        {p.grain && p.grain > 0.01 ? (
          <div className="grain-overlay" style={{ opacity: Math.min(0.5, p.grain * 0.5) }} />
        ) : null}
        {p.vignette && p.vignette > 0.01 ? (
          <div className="vignette-overlay" style={{ opacity: Math.min(1, p.vignette) }} />
        ) : null}
        {p.censors && p.censors.length > 0 && p.onCensor && (
          <CensorLayer
            censors={p.censors}
            resolution={p.resolution}
            frameSrc={p.layers.length ? p.layers[p.layers.length - 1].src : null}
            onCensor={p.onCensor}
          />
        )}
        {safe && (
          <>
            <div className="safe action" />
            <div className="safe title" />
          </>
        )}
        {p.titleOverlay}
        {p.caption && <div className="caption">{p.caption}</div>}
      </div>
      <div className="transport">
        <IconButton label="Jump to start" hint="Home" onClick={() => p.onSeek(0)}>
          ⏮
        </IconButton>
        <IconButton
          label={p.playing ? "Pause" : "Play"}
          hint="Space"
          onClick={p.onTogglePlay}
          disabled={!p.canPlay}
          active={p.playing}
        >
          {p.playing ? "❚❚" : "▶"}
        </IconButton>
        <IconButton label="Jump to end" hint="End" onClick={() => p.onSeek(p.contentEnd)}>
          ⏭
        </IconButton>
        <span className="tc" title="Playhead timecode">
          {formatTC(p.playhead)}
        </span>
        <span className="spacer" />
        <select
          className="res-select"
          title="Export resolution"
          value={p.resolution.label}
          onChange={(e) =>
            p.onResolution(RESOLUTIONS.find((r) => r.label === e.target.value) ?? RESOLUTIONS[1])
          }
        >
          {RESOLUTIONS.map((r) => (
            <option key={r.label}>{r.label}</option>
          ))}
        </select>
        <IconButton label="Safe margins" active={safe} onClick={() => setSafe((s) => !s)}>
          ⛶
        </IconButton>
        <IconButton
          label="Fullscreen"
          onClick={() => frameRef.current?.requestFullscreen?.().catch(() => {})}
        >
          ⤢
        </IconButton>
      </div>
    </section>
  );
}
