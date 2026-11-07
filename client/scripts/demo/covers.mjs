// Procedural demo covers ("night posters"), drawn by code, no AI and no image inputs. Fictional titles only.
// Run from scripts/demo after npm ci: node covers.mjs [slug...]
//   -> app/src/debug/assets/demo/art/N.jpg (N: the game's place in DemoConsole.LIBRARY)
//   -> out/demo/assets/covers-contact.png, every cover cropped as a 100 % desktop tile
import { chromium } from 'playwright-core';
import { mkdirSync, rmSync, writeFileSync, readFileSync, readdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const client = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function rng(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

class Art {
  constructor(w = 600, h = 900, seed = 1) {
    this.w = w; this.h = h; this.defs = []; this.body = []; this.n = 0; this.r = rng(seed);
  }
  id() { return 'i' + this.n++; }
  stops(stops) {
    return stops.map(([o, c, a]) => `<stop offset="${o}" stop-color="${c}"${a === undefined ? '' : ` stop-opacity="${a}"`}/>`).join('');
  }
  lin(stops, x1 = 0, y1 = 0, x2 = 0, y2 = 1) {
    const id = this.id();
    this.defs.push(`<linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="${x1 * this.w}" y1="${y1 * this.h}" x2="${x2 * this.w}" y2="${y2 * this.h}">${this.stops(stops)}</linearGradient>`);
    return `url(#${id})`;
  }
  rad(cx, cy, r, stops) {
    const id = this.id();
    this.defs.push(`<radialGradient id="${id}" gradientUnits="userSpaceOnUse" cx="${cx}" cy="${cy}" r="${r}">${this.stops(stops)}</radialGradient>`);
    return `url(#${id})`;
  }
  add(s) { this.body.push(s); return this; }
  bg(fill) { return this.add(`<rect width="${this.w}" height="${this.h}" fill="${fill}"/>`); }
  rect(x, y, w, h, fill, op = 1, rx = 0) { return this.add(`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="${fill}" opacity="${op}"/>`); }
  circle(x, y, r, fill, op = 1) { return this.add(`<circle cx="${x}" cy="${y}" r="${r}" fill="${fill}" opacity="${op}"/>`); }
  glow(x, y, r, c, op = 1) { return this.circle(x, y, r, this.rad(x, y, r, [[0, c, 1], [0.35, c, 0.45], [1, c, 0]]), op); }
  path(d, fill, op = 1, extra = '') { return this.add(`<path d="${d}" fill="${fill}" opacity="${op}" ${extra}/>`); }
  line(d, stroke, width, op = 1, extra = '') { return this.add(`<path d="${d}" fill="none" stroke="${stroke}" stroke-width="${width}" stroke-linecap="round" stroke-linejoin="round" opacity="${op}" ${extra}/>`); }
  blur(std) {
    const id = this.id();
    this.defs.push(`<filter id="${id}" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="${std}"/></filter>`);
    return `filter="url(#${id})"`;
  }
  // Fractal ridge line across the width, closed down to the bottom.
  ridge(base, amp, rough = 0.55, step = 7, r = this.r) {
    const n = 1 << step; const ys = new Array(n + 1).fill(0);
    ys[0] = (r() - 0.5) * amp; ys[n] = (r() - 0.5) * amp;
    let scale = amp;
    for (let size = n; size > 1; size >>= 1) {
      for (let i = size >> 1; i < n; i += size) ys[i] = (ys[i - (size >> 1)] + ys[i + (size >> 1)]) / 2 + (r() - 0.5) * scale;
      scale *= rough;
    }
    const pts = ys.map((y, i) => [(i / n) * this.w, base + y]);
    this.lastRidge = pts;
    return `M0,${this.h} ` + pts.map((p) => `L${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' ') + ` L${this.w},${this.h} Z`;
  }
  // Smooth rolling hill.
  hill(base, parts) {
    const pts = [];
    for (let i = 0; i <= 80; i++) {
      const x = (i / 80) * this.w; let y = base;
      for (const [a, f, p] of parts) y += a * Math.sin((x / this.w) * f * Math.PI * 2 + p);
      pts.push([x, y]);
    }
    this.lastRidge = pts;
    return `M0,${this.h} ` + pts.map((p) => `L${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' ') + ` L${this.w},${this.h} Z`;
  }
  stars(n, maxY, c = '#fff', big = 1.6) {
    for (let i = 0; i < n; i++) this.circle((this.r() * this.w).toFixed(1), (this.r() * maxY).toFixed(1), (0.4 + this.r() ** 3 * big).toFixed(2), c, (0.25 + this.r() * 0.75).toFixed(2));
    return this;
  }
  text(lines, { x = 48, y = 110, size = 84, lead = 0.96, fill = '#fff', anchor = 'start', track = 0, weight = 600, skew = 0, heavy = 0, op = 1, shadow = null } = {}) {
    let o = '';
    lines.forEach((t, i) => {
      const yy = y + i * size * lead;
      const tf = skew ? ` transform="translate(${x} ${yy}) skewX(${skew})"` : ` x="${x}" y="${yy}"`;
      const st = heavy ? ` stroke="${fill}" stroke-width="${heavy}" stroke-linejoin="round" paint-order="stroke"` : '';
      const sh = shadow ? ` style="filter:drop-shadow(${shadow})"` : '';
      o += `<text${tf} font-family="Figtree" font-weight="${weight}" font-size="${size}" fill="${fill}" text-anchor="${anchor}" letter-spacing="${track}" opacity="${op}"${st}${sh}>${t}</text>`;
    });
    return this.add(o);
  }
  // Fine grain and a soft vignette, the two things that make flat vector read as a printed poster.
  finish({ grain = 0.14, vignette = 0.35 } = {}) {
    if (vignette) this.bg(this.rad(this.w / 2, this.h * 0.45, Math.max(this.w, this.h) * 0.78, [[0.55, '#000', 0], [1, '#000', vignette]]));
    if (grain) {
      const id = this.id();
      this.defs.push(`<filter id="${id}" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" seed="7"/><feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 1.6 -0.55"/></filter>`);
      this.add(`<rect width="${this.w}" height="${this.h}" filter="url(#${id})" opacity="${grain}" style="mix-blend-mode:soft-light"/>`);
    }
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${this.w}" height="${this.h}" viewBox="0 0 ${this.w} ${this.h}"><defs>${this.defs.join('')}</defs>${this.body.join('')}</svg>`;
  }
}

const scenes = {};

// ---------------------------------------------------------------- Racing: dusk highway, tail lights.
// The demo's running game, so every hero and the showcase images open on it: a full 2:3 cover, the title starting below
// the Running badge in a tile's top corner.
scenes['racing-game'] = () => {
  const w = 600; const h = 900;
  const a = new Art(w, h, 12);
  const hz = h * 0.56;
  const hzF = hz / h;
  a.bg(a.lin([[0, '#14092c'], [hzF * 0.42, '#4b1160'], [hzF * 0.72, '#c5275c'], [hzF * 0.9, '#ff6d3a'], [hzF, '#ffc56b'], [1, '#ffc56b']]));
  a.stars(90, hz * 0.5, '#ffe9f4', 1.4);
  const vx = w * 0.56;
  const u = Math.min(w, h * 0.667);
  a.glow(vx, hz, u * 0.92, '#ffb36b', 0.8);
  a.circle(vx, hz - u * 0.13, u * 0.2, a.rad(vx, hz - u * 0.13, u * 0.2, [[0, '#fff6d6'], [0.72, '#ffd98a'], [1, '#ff9d4f']]));
  a.path(a.ridge(hz - h * 0.035, h * 0.075, 0.56), '#6d1a5e', 0.9);
  a.path(a.ridge(hz - h * 0.008, h * 0.05, 0.6), '#2e0e3d');
  // Road in perspective with a sweeping bend.
  const steps = 60; const L = []; const R = []; const C = [];
  const curve = (t) => vx + (w * 0.5 - vx) * t + Math.sin(t * Math.PI) * w * 0.05 * (1 - t);
  for (let i = 0; i <= steps; i++) {
    const t = i / steps; const y = hz + (h - hz) * t ** 1.5 + 0.5;
    const half = (w * 0.012 + w * 0.86 * t ** 1.5);
    L.push([curve(t) - half, y]); R.push([curve(t) + half, y]); C.push([curve(t), y, half]);
  }
  const poly = (l, r) => 'M' + l.map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' L') + ' L' + [...r].reverse().map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' L') + ' Z';
  a.path(`M0,${hz} L${w},${hz} L${w},${h} L0,${h} Z`, a.lin([[hzF, '#1b0a24'], [1, '#07050c']]));
  a.path(poly(L, R), a.lin([[hzF, '#6a2a4e'], [hzF + (1 - hzF) * 0.25, '#2a1230'], [1, '#100a16']]));
  // Road sheen from the sun.
  a.path(poly(C.map((p) => [p[0] - p[2] * 0.42, p[1]]), C.map((p) => [p[0] + p[2] * 0.42, p[1]])), a.lin([[hzF, '#ffb36b', 0.75], [hzF + (1 - hzF) * 0.6, '#ff6d3a', 0.06], [1, '#ff6d3a', 0]]));
  // Edge lines and light trails, each a ribbon that widens toward the viewer.
  const ribbon = (off, wid, color, op, from = 0) => {
    const l = []; const r = [];
    for (const p of C.slice(from)) { const cx = p[0] + p[2] * off; const hw = Math.max(0.6, p[2] * wid); l.push([cx - hw, p[1]]); r.push([cx + hw, p[1]]); }
    return [poly(l, r), color, op];
  };
  const trails = [
    ribbon(-0.97, 0.012, '#ffe1b8', 0.95), ribbon(0.97, 0.012, '#ffe1b8', 0.95),
    ribbon(-0.62, 0.016, '#ff2b4e', 1), ribbon(-0.57, 0.009, '#ff8a6e', 1),
    ribbon(0.56, 0.018, '#ffffff', 0.95), ribbon(0.62, 0.009, '#ffd9a0', 0.9),
    ribbon(-0.28, 0.012, '#ff3b5c', 0.9, 6), ribbon(0.27, 0.01, '#fff1d0', 0.8, 10),
  ];
  const glow = a.blur(u * 0.016);
  for (const [d, c, op] of trails) a.path(d, c, op * 0.85, glow);
  for (const [d, c, op] of trails) a.path(d, c, op);
  // Dashes.
  for (let i = 8; i < steps - 2; i += 6) {
    const s = C[i]; const e = C[Math.min(i + 3, steps)];
    a.path(`M${s[0] - s[2] * 0.012},${s[1]} L${s[0] + s[2] * 0.012},${s[1]} L${e[0] + e[2] * 0.012},${e[1]} L${e[0] - e[2] * 0.012},${e[1]} Z`, '#ffe9c7', 0.9);
  }
  // Car from behind: low wedge, tail-light bar.
  const cs = u * 0.46; const cx = w * 0.5; const cy = h * 0.84;
  const X = (v) => (cx + v * cs).toFixed(1); const Y = (v) => (cy + v * cs).toFixed(1);
  a.add(`<ellipse cx="${cx}" cy="${Y(0.42)}" rx="${cs * 0.72}" ry="${cs * 0.08}" fill="#000" opacity=".75" ${a.blur(cs * 0.03)}/>`);
  a.rect(X(-0.54), Y(0.24), cs * 0.2, cs * 0.18, '#060409', 1, cs * 0.03);
  a.rect(X(0.34), Y(0.24), cs * 0.2, cs * 0.18, '#060409', 1, cs * 0.03);
  a.path(`M${X(-0.6)},${Y(0.3)} C${X(-0.62)},${Y(0.1)} ${X(-0.6)},${Y(0.0)} ${X(-0.5)},${Y(-0.05)} L${X(-0.32)},${Y(-0.3)} C${X(-0.14)},${Y(-0.36)} ${X(0.14)},${Y(-0.36)} ${X(0.32)},${Y(-0.3)} L${X(0.5)},${Y(-0.05)} C${X(0.6)},${Y(0.0)} ${X(0.62)},${Y(0.1)} ${X(0.6)},${Y(0.3)} Z`, a.lin([[(cy - 0.36 * cs) / h, '#3a1a42'], [(cy + 0.3 * cs) / h, '#0b0710']]));
  a.path(`M${X(-0.42)},${Y(-0.06)} L${X(-0.27)},${Y(-0.26)} C${X(-0.1)},${Y(-0.3)} ${X(0.1)},${Y(-0.3)} ${X(0.27)},${Y(-0.26)} L${X(0.42)},${Y(-0.06)} Z`, a.lin([[(cy - 0.3 * cs) / h, '#ff9a5a', 0.9], [(cy - 0.06 * cs) / h, '#3a1533', 1]]));
  const lb = a.blur(cs * 0.035);
  a.rect(X(-0.52), Y(0.03), cs * 1.04, cs * 0.05, '#ff1f42', 1, cs * 0.02);
  a.add(`<rect x="${X(-0.56)}" y="${Y(0.01)}" width="${cs * 1.12}" height="${cs * 0.09}" fill="#ff2b4e" ${lb}/>`);
  a.rect(X(-0.5), Y(0.042), cs, cs * 0.018, '#ffd0c8', 1);
  a.rect(X(-0.16), Y(0.16), cs * 0.32, cs * 0.07, '#17101c', 1, cs * 0.012);
  a.text(['RACING', 'GAME'], { x: 50, y: 150, size: 116, lead: 0.92, skew: -11, heavy: 2, track: -1, fill: '#fff6ea' });
  return a.finish();
};

// ---------------------------------------------------------------- Open world: layered ridges at dawn, a square.
scenes['open-world'] = () => {
  const a = new Art(600, 600, 41);
  a.bg(a.lin([[0, '#0c2f4c'], [0.26, '#1b6a86'], [0.44, '#6fb5a8'], [0.56, '#f5c98a'], [0.66, '#ff9f5c'], [1, '#ff9f5c']]));
  a.glow(370, 310, 250, '#fff0c0', 0.95);
  a.circle(370, 310, 46, '#fff8e0');
  const ridges = [[360, 54, '#d98c6a'], [393, 74, '#a4716f'], [427, 74, '#5f6a78'], [467, 66, '#2c5365'], [503, 60, '#12394a']];
  ridges.forEach(([b, amp, c]) => {
    a.path(a.ridge(b, amp, 0.58), c);
    a.bg(a.lin([[(b + 26) / 600, '#ffd9a0', 0.1], [(b + 100) / 600, '#ffd9a0', 0]]));
  });
  // Conifers on the last ridge.
  a.path(a.ridge(541, 36, 0.5), '#051b26');
  const ridge = a.lastRidge;
  let d = '';
  for (let i = 0; i < 70; i++) {
    const x = a.r() * 600; const p = ridge[Math.round((x / 600) * (ridge.length - 1))];
    const ht = 24 + a.r() * 42; const wd = ht * 0.2;
    d += `M${x},${p[1] - ht} L${x + wd},${p[1] + 6} L${x - wd},${p[1] + 6} Z `;
  }
  a.path(d, '#051b26');
  // A flock crossing the sun.
  let fl = '';
  [[334, 286, 1], [358, 266, 0.8], [310, 268, 0.7], [382, 298, 0.62], [288, 292, 0.55]].forEach(([x, y, s]) => {
    fl += `M${x - 14 * s},${y - 3 * s} Q${x - 6 * s},${y - 8 * s} ${x},${y} Q${x + 6 * s},${y - 8 * s} ${x + 14 * s},${y - 3 * s} `;
  });
  a.line(fl, '#0c2f4c', 2.6);
  // Clients crop the square to the middle: the title stays inside that.
  a.text(['Open', 'World'], { x: 300, y: 104, size: 92, lead: 0.9, heavy: 1.2, track: -2, fill: '#fffaf0', anchor: 'middle' });
  return a.finish({ vignette: 0.22 });
};

// ---------------------------------------------------------------- Ocean: light shafts, reef, a little sub.
scenes['ocean-explorer'] = () => {
  const a = new Art(600, 900, 77);
  a.bg(a.lin([[0, '#b9f6ea'], [0.12, '#58d8d4'], [0.36, '#118ea6'], [0.66, '#07476f'], [1, '#021426']]));
  // Surface caustic band.
  a.glow(300, -40, 520, '#f4fff6', 0.9);
  for (let i = 0; i < 9; i++) {
    const x = 30 + i * 66 + a.r() * 30; const wd = 18 + a.r() * 46; const lean = 90 + a.r() * 80;
    a.path(`M${x},-10 L${x + wd},-10 L${x + wd + lean},820 L${x + lean - wd * 0.8},820 Z`, a.lin([[0, '#eafff7', 0.34], [0.72, '#c8fff0', 0.02], [0.9, '#c8fff0', 0]]));
  }
  // Fish school.
  let fish = '';
  for (let i = 0; i < 46; i++) {
    const t = a.r(); const x = 90 + t * 300 + a.r() * 70; const y = 400 + Math.sin(t * 6) * 44 + a.r() * 60; const s = 5 + a.r() * 5;
    fish += `M${x},${y} q${s},${-s * 0.55} ${s * 2.2},0 q${-s * 1.2},${s * 0.55} ${-s * 2.2},0 Z M${x},${y} l${-s * 0.8},${-s * 0.42} l0,${s * 0.84} Z `;
  }
  a.path(fish, '#032f4a', 0.82);
  // Sub with a beam.
  const sx = 386; const sy = 556;
  a.path(`M${sx - 62},${sy + 4} L${sx - 330},${sy + 150} L${sx - 300},${sy - 46} Z`, a.lin([[0, '#fff4c2', 0], [1, '#fff4c2', 0]], 0, 0, 1, 0));
  a.path(`M${sx - 60},${sy + 4} L${sx - 360},${sy - 54} L${sx - 360},${sy + 150} Z`, (() => { const id = a.id(); a.defs.push(`<linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="${sx - 60}" y1="0" x2="${sx - 360}" y2="0"><stop offset="0" stop-color="#fff2b5" stop-opacity=".78"/><stop offset="1" stop-color="#fff2b5" stop-opacity="0"/></linearGradient>`); return `url(#${id})`; })());
  a.add(`<ellipse cx="${sx}" cy="${sy}" rx="66" ry="34" fill="#f2a93b"/>`);
  a.add(`<path d="M${sx - 66},${sy} A66,34 0 0 0 ${sx + 66},${sy} Z" fill="#b76a14"/>`);
  a.rect(sx - 20, sy - 50, 40, 22, '#e8962a', 1, 7);
  a.rect(sx + 62, sy - 18, 24, 36, '#b76a14', 1, 6);
  a.circle(sx - 38, sy - 4, 13, '#063049'); a.circle(sx - 38, sy - 4, 8.5, '#bff5ea');
  a.circle(sx - 2, sy - 4, 9, '#063049'); a.circle(sx - 2, sy - 4, 5.5, '#bff5ea');
  a.circle(sx - 66, sy + 2, 6, '#fff2b5');
  for (let i = 0; i < 16; i++) { const r = 2 + a.r() * 5; a.add(`<circle cx="${sx + 88 + a.r() * 40}" cy="${sy - 30 - i * 19 - a.r() * 8}" r="${r}" fill="none" stroke="#d6fff4" stroke-width="1.3" opacity="${0.75 - i * 0.03}"/>`); }
  // Reef layers and kelp.
  a.path(a.ridge(748, 70, 0.62), '#053a58');
  a.path(a.ridge(806, 70, 0.66), '#03233c');
  for (let i = 0; i < 12; i++) {
    const x = 24 + a.r() * 552; const ht = 120 + a.r() * 170; const wob = 16 + a.r() * 22; const ph = a.r() * 6;
    let d = `M${x},900`;
    for (let k = 1; k <= 8; k++) d += ` L${(x + Math.sin(ph + k * 0.9) * wob * (k / 8)).toFixed(1)},${(900 - (ht * k) / 8).toFixed(1)}`;
    a.line(d, '#021a2d', 5 + a.r() * 5, 1);
  }
  a.path(a.ridge(868, 34, 0.6), '#01101f');
  a.text(['OCEAN'], { x: 300, y: 128, size: 112, anchor: 'middle', track: 8, heavy: 1, fill: '#032b44' });
  a.text(['EXPLORER'], { x: 300, y: 176, size: 36, anchor: 'middle', track: 17, fill: '#032b44' });
  return a.finish({ vignette: 0.25 });
};

// ---------------------------------------------------------------- Music: a groove of ribbons and a big record.
scenes['music-studio'] = () => {
  const a = new Art(600, 900, 23);
  a.bg(a.lin([[0, '#1a0a3d'], [0.5, '#4a1272'], [1, '#d42c7f']], 0, 0, 0.6, 1));
  a.glow(150, 660, 430, '#ff5ca8', 0.7);
  a.glow(520, 190, 340, '#7d4dff', 0.8);
  const cx = 420; const cy = 300;
  a.circle(cx, cy, 262, a.rad(cx, cy, 262, [[0, '#120726'], [0.82, '#1a0b34'], [1, '#2d1353']]));
  for (let r = 92; r < 258; r += 6) a.add(`<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="#fff" stroke-width=".8" opacity="${0.03 + (r % 24 === 0 ? 0.07 : 0.02)}"/>`);
  // Light on the grooves: two opposed wedges.
  a.path(`M${cx},${cy} L${cx + 250},${cy - 160} A296,296 0 0 1 ${cx + 280},${cy - 70} Z`, '#ffd1ec', 0.16);
  a.path(`M${cx},${cy} L${cx - 250},${cy + 160} A296,296 0 0 1 ${cx - 280},${cy + 70} Z`, '#ffd1ec', 0.12);
  a.circle(cx, cy, 84, a.lin([[0.2, '#ffbf5e'], [0.5, '#ff5ca8']], 0, 0, 0, 1));
  a.circle(cx, cy, 11, '#120726');
  // Wave ribbons.
  const N = 34;
  for (let i = 0; i < N; i++) {
    const t = i / (N - 1); const base = 500 + t * 330;
    let d = '';
    for (let k = 0; k <= 120; k++) {
      const x = (k / 120) * 620 - 10; const u = x / 600;
      const env = Math.exp(-((u - 0.42) ** 2) / 0.05);
      const y = base - env * (130 * (1 - t * 0.55)) * (0.65 + 0.35 * Math.sin(u * 19 + t * 5)) - Math.sin(u * 6 + t * 2.2) * 16;
      d += (k ? ' L' : 'M') + x.toFixed(1) + ',' + y.toFixed(1);
    }
    a.path(d + ` L610,900 L-10,900 Z`, '#150730', 0.26);
    a.line(d, a.lin([[0, '#ff7ab8'], [0.5, '#ffd27a'], [1, '#8f7dff']], 0, 0, 1, 0), 1.7, 0.4 + t * 0.6);
  }
  a.text(['music', 'studio'], { x: 40, y: 780, size: 108, lead: 0.88, heavy: 1.5, track: -3, fill: '#fff5fb' });
  return a.finish({ vignette: 0.28 });
};

// ---------------------------------------------------------------- Space sim: planet, ring, ship.
scenes['space-sim'] = () => {
  const a = new Art(600, 900, 5);
  a.bg(a.lin([[0, '#03040d'], [0.6, '#0a0d2c'], [1, '#160b37']]));
  a.glow(120, 240, 420, '#3f2fb5', 0.5); a.glow(540, 700, 420, '#c2389a', 0.32);
  a.stars(420, 900, '#dfe6ff', 1.8);
  const cx = 360; const cy = 500; const R = 230;
  const backRing = `<g transform="rotate(-20 ${cx} ${cy})">` + [[R * 1.72, 2], [R * 1.6, 11], [R * 1.46, 18], [R * 1.33, 5]].map(([rx, sw], i) => `<path d="M${cx - rx},${cy} A${rx},${rx * 0.2} 0 0 1 ${cx + rx},${cy}" fill="none" stroke="#e9c9a1" stroke-width="${sw}" opacity="${0.35 + i * 0.1}"/>`).join('') + '</g>';
  a.add(backRing);
  const id = a.id(); a.defs.push(`<clipPath id="${id}"><circle cx="${cx}" cy="${cy}" r="${R}"/></clipPath>`);
  a.circle(cx, cy, R, a.lin([[0.36, '#f6c690'], [0.48, '#d9774f'], [0.6, '#7a2c54'], [0.76, '#22103a']], 0, 0, 0.9, 0.6));
  let bands = '';
  for (let i = -8; i < 9; i++) bands += `<rect x="${cx - R - 40}" y="${cy + i * 28}" width="${R * 2 + 80}" height="${5 + a.r() * 12}" fill="${i % 2 ? '#ffe6bd' : '#3c1036'}" opacity="${0.08 + a.r() * 0.14}" transform="rotate(-20 ${cx} ${cy})"/>`;
  a.add(`<g clip-path="url(#${id})">${bands}<circle cx="${cx + 110}" cy="${cy + 96}" r="${R * 1.05}" fill="${a.rad(cx + 110, cy + 96, R * 1.05, [[0.25, '#04030c', 0.92], [1, '#04030c', 0]])}"/></g>`);
  a.add(`<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="#ffd9a8" stroke-width="1.5" opacity=".35"/>`);
  a.add(`<g transform="rotate(-20 ${cx} ${cy})">` + [[R * 1.72, 2], [R * 1.6, 11], [R * 1.46, 18], [R * 1.33, 5]].map(([rx, sw], i) => `<path d="M${cx - rx},${cy} A${rx},${rx * 0.2} 0 0 0 ${cx + rx},${cy}" fill="none" stroke="#f6ddb9" stroke-width="${sw}" opacity="${0.45 + i * 0.12}"/>`).join('') + '</g>');
  // Ship and drive plume.
  const sx = 142; const sy = 738;
  a.path(`M${sx},${sy} L${sx - 150},${sy + 74} L${sx - 150},${sy + 92} Z`, a.lin([[0, '#7fe0ff', 0.9], [1, '#7fe0ff', 0]], sx / 600, 0, (sx - 150) / 600, 0.0));
  a.glow(sx - 4, sy + 4, 30, '#9ae8ff', 0.95);
  a.path(`M${sx - 8},${sy - 8} L${sx + 66},${sy - 44} L${sx + 38},${sy - 6} L${sx + 8},${sy + 16} Z`, '#f4efe6');
  a.path(`M${sx + 66},${sy - 44} L${sx + 38},${sy - 6} L${sx + 8},${sy + 16} L${sx + 26},${sy - 12} Z`, '#8d93b5');
  a.text(['SPACE SIM'], { x: 300, y: 120, size: 62, anchor: 'middle', track: 15, weight: 400, fill: '#f4efe6' });
  a.rect(258, 146, 84, 2, '#f2a93b');
  return a.finish({ vignette: 0.2 });
};

// ---------------------------------------------------------------- Puzzle: isometric blocks, one out of place.
scenes['puzzle-night'] = () => {
  const a = new Art(600, 900, 9);
  a.bg(a.lin([[0, '#14123f'], [0.55, '#2a1f6d'], [1, '#6a3aa6']]));
  a.glow(300, 560, 480, '#7a64ff', 0.6);
  a.stars(70, 340, '#fff', 1.2);
  const cube = (cx, cy, s, top, left, right) => {
    const dx = s * 0.866; const dy = s * 0.5;
    a.path(`M${cx},${cy - s} L${cx + dx},${cy - dy} L${cx},${cy} L${cx - dx},${cy - dy} Z`, top);
    a.path(`M${cx - dx},${cy - dy} L${cx},${cy} L${cx},${cy + s} L${cx - dx},${cy + dy} Z`, left);
    a.path(`M${cx + dx},${cy - dy} L${cx},${cy} L${cx},${cy + s} L${cx + dx},${cy + dy} Z`, right);
  };
  const S = 64; const dx = S * 0.866; const dy = S * 0.5;
  const pal = [['#b5a8ff', '#5240c2', '#2e2285'], ['#8d7cf2', '#3f2fa8', '#231a6e'], ['#d5ccff', '#6853d8', '#3a2c9c']];
  const heights = [[3, 2, 3, 1], [2, 3, 1, 2], [1, 2, 2, 0], [2, 1, 0, 1]];
  const ox = 300; const oy = 520;
  for (let gx = 0; gx < 4; gx++) for (let gy = 0; gy < 4; gy++) {
    const h = heights[gx][gy];
    const x = ox + (gx - gy) * dx; const baseY = oy + (gx + gy) * dy;
    for (let k = 0; k <= h; k++) {
      const p = pal[(gx + gy + k) % 3];
      cube(x, baseY - k * S, S, p[0], p[1], p[2]);
    }
  }
  // Glowing piece in the gap.
  const gx = ox + (2 - 3) * dx; const gy = oy + (2 + 3) * dy - 3.0 * S;
  a.glow(gx, gy, 230, '#ffc865', 0.9);
  cube(gx, gy, S, '#fff0c2', '#f2a93b', '#c97c18');
  a.text(['Puzzle', 'Night'], { x: 44, y: 130, size: 104, lead: 0.92, heavy: 1.5, track: -2, fill: '#f6f1ff' });
  return a.finish({ vignette: 0.28 });
};

// ---------------------------------------------------------------- Retro arcade: real pixels from a 5x7 font.
const FONT = {
  R: ['11110', '10001', '10001', '11110', '10100', '10010', '10001'], E: ['11111', '10000', '10000', '11110', '10000', '10000', '11111'],
  T: ['11111', '00100', '00100', '00100', '00100', '00100', '00100'], O: ['01110', '10001', '10001', '10001', '10001', '10001', '01110'],
  A: ['01110', '10001', '10001', '11111', '10001', '10001', '10001'], C: ['01110', '10001', '10000', '10000', '10000', '10001', '01110'],
  D: ['11110', '10001', '10001', '10001', '10001', '10001', '11110'],
};
scenes['retro-arcade'] = () => {
  const a = new Art(600, 900, 31);
  const p = 10;
  a.bg(a.lin([[0, '#14062e'], [0.55, '#3b0c5e'], [1, '#c01e6f']]));
  const word = (t, x, y, px, c, sh) => {
    [...t].forEach((ch, i) => FONT[ch].forEach((row, ry) => [...row].forEach((b, rx) => {
      if (b === '1') { const X = x + (rx + i * 6) * px; const Y = y + ry * px; a.rect(X + px * 0.5, Y + px * 0.5, px, px, sh); a.rect(X, Y, px, px, c); }
    })));
  };
  // Pixel stars.
  for (let i = 0; i < 60; i++) { const s = a.r() > 0.82 ? p : p / 2; a.rect(Math.floor(a.r() * 60) * p, Math.floor(a.r() * 44) * p, s, s, a.r() > 0.5 ? '#ffe36b' : '#7ef2ff', 0.8); }
  // A pixel joystick, big: no sprite from any real game.
  const stick = ['00000rrr00000', '0000rhrrr0000', '0000rrrrr0000', '00000rrr00000', '000000w000000', '000000w000000', '000000w000000', '00000ccc00000', '000ccccccc000', '00ccccccccc00'];
  const ip = 32; const ix = 300 - (13 * ip) / 2; const iy = 330;
  const stickColour = { r: '#ff5c8a', h: '#ffd5e2', w: '#fff6ea', c: a.lin([[iy / 900, '#7ef2ff'], [(iy + 10 * ip) / 900, '#37e0a8']]) };
  a.glow(300, iy + 130, 340, '#ff3fa4', 0.75);
  stick.forEach((row, ry) => [...row].forEach((b, rx) => { if (b !== '0') a.rect(ix + rx * ip, iy + ry * ip, ip, ip, stickColour[b]); }));
  // The control panel and its three buttons.
  a.rect(40, 690, 520, 20, '#fff6ea', 0.85);
  a.rect(40, 710, 520, 130, '#24093f');
  for (const [bx, c] of [[150, '#ffe36b'], [300, '#7ef2ff'], [450, '#37e0a8']]) {
    for (const [i, j] of [[1, 0], [0, 1], [1, 1], [2, 1], [1, 2]]) a.rect(bx - 36 + i * 24, 738 + j * 24, 24, 24, c);
  }
  word('RETRO', 300 - (5 * 6 * 16 - 16) / 2, 70, 16, '#ffe36b', '#d6186d');
  word('ARCADE', 300 - (6 * 6 * 13 - 13) / 2, 202, 13, '#fff6ea', '#7a1fb0');
  // Scanlines.
  for (let y = 0; y < 900; y += 4) a.rect(0, y, 600, 1.4, '#000', 0.16);
  return a.finish({ grain: 0.06, vignette: 0.42 });
};

// ---------------------------------------------------------------- Ember Keep: castle on a crag, embers rising.
scenes['ember-keep'] = () => {
  const a = new Art(600, 900, 66);
  a.bg(a.lin([[0, '#1a0608'], [0.3, '#5c1210'], [0.56, '#d4421a'], [0.74, '#ffa63d'], [1, '#ffa63d']]));
  a.glow(300, 470, 430, '#ffc05c', 0.9);
  a.circle(300, 440, 150, a.rad(300, 440, 150, [[0, '#fff2c4'], [0.8, '#ffc765'], [1, '#ff9440']]));
  a.path(a.ridge(640, 80, 0.56), '#8c2614');
  a.path(a.ridge(700, 80, 0.56), '#4f130e');
  // Crag and castle.
  const k = '#16060a';
  a.path(`M60,900 L120,720 L170,690 L210,620 L270,596 L340,600 L400,640 L440,700 L520,760 L600,900 Z`, k);
  const tower = (x, top, w) => {
    a.rect(x - w / 2, top, w, 700 - top, k);
    const m = w / 5;
    for (let i = 0; i < 3; i++) a.rect(x - w / 2 + i * 2 * m, top - m * 1.2, m, m * 1.2, k);
  };
  tower(226, 470, 48); tower(366, 500, 44); tower(296, 396, 62);
  a.rect(226, 548, 150, 120, k);
  for (let i = 0; i < 8; i++) a.rect(230 + i * 19, 536, 11, 14, k);
  a.path(`M296,330 L296,396 M296,332 L340,346 L296,360`, 'none', 1, `stroke="${k}" stroke-width="4"`);
  a.path(`M296,333 L342,347 L296,361 Z`, k);
  for (const [x, y, w, h] of [[292, 444, 9, 22], [222, 508, 7, 18], [362, 534, 7, 18], [260, 594, 7, 16], [330, 594, 7, 16]]) { a.rect(x, y, w, h, '#ffb347'); a.glow(x + w / 2, y + h / 2, 24, '#ff8a2a', 0.8); }
  a.path(a.ridge(830, 40, 0.5), '#080305');
  // Embers.
  for (let i = 0; i < 90; i++) { const x = a.r() * 600; const y = 240 + a.r() * 660; const r = 0.8 + a.r() ** 2 * 3; a.circle(x, y, r, a.r() > 0.5 ? '#ffd27a' : '#ff6a2a', 0.5 + a.r() * 0.5); }
  a.text(['EMBER KEEP'], { x: 300, y: 118, size: 72, anchor: 'middle', track: 5, heavy: 1.5, fill: '#fff0d6' });
  a.rect(150, 142, 300, 2, '#ffc765', 0.8);
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Frostline: high key low-poly ice (bright stress case).
scenes['frostline'] = () => {
  const a = new Art(600, 900, 88);
  const cols = 8; const rows = 12; const pts = [];
  for (let y = 0; y <= rows; y++) { pts[y] = []; for (let x = 0; x <= cols; x++) pts[y][x] = [(x / cols) * 600 + (x % cols ? (a.r() - 0.5) * 50 : 0), (y / rows) * 900 + (y % rows ? (a.r() - 0.5) * 50 : 0)]; }
  const mix = (c1, c2, t) => { const h = (c, i) => parseInt(c.slice(1 + i * 2, 3 + i * 2), 16); const v = [0, 1, 2].map((i) => Math.round(h(c1, i) + (h(c2, i) - h(c1, i)) * t)); return `rgb(${v.join(',')})`; };
  for (let y = 0; y < rows; y++) for (let x = 0; x < cols; x++) {
    const [p, q, r, s] = [pts[y][x], pts[y][x + 1], pts[y + 1][x + 1], pts[y + 1][x]];
    for (const tri of [[p, q, r], [p, r, s]]) {
      const cy = (tri[0][1] + tri[1][1] + tri[2][1]) / 3 / 900; const cx = (tri[0][0] + tri[1][0] + tri[2][0]) / 3 / 600;
      const light = Math.max(0, Math.min(1, cy * 1.15 + (cx - 0.5) * 0.28 + (a.r() - 0.5) * 0.22));
      const c = light < 0.5 ? mix('#ffffff', '#bfe9ff', light * 2) : mix('#bfe9ff', '#2d7ec4', (light - 0.5) * 2);
      a.path(`M${tri[0]} L${tri[1]} L${tri[2]} Z`, c, 1, `stroke="${c}" stroke-width=".6"`);
    }
  }
  // A lone figure and a ridge line.
  a.path(a.ridge(790, 60, 0.45), '#114a86', 0.92);
  a.path(a.ridge(840, 40, 0.4), '#0a2c5c');
  a.text(['Frost', 'line'], { x: 44, y: 148, size: 134, lead: 0.86, heavy: 1.5, track: -4, fill: '#0c3263' });
  return a.finish({ vignette: 0, grain: 0.05 });
};

// ---------------------------------------------------------------- Brightwater: sunlit lagoon (bright stress case).
scenes['brightwater'] = () => {
  const a = new Art(600, 900, 16);
  a.bg(a.lin([[0, '#fffbe8'], [0.34, '#fff3c0'], [0.48, '#bdf0e4'], [0.72, '#35c6c8'], [1, '#0b7f9e']]));
  a.glow(150, 190, 300, '#ffffff', 1);
  a.circle(150, 190, 76, '#fff');
  a.path(a.hill(430, [[22, 1.2, 1], [10, 3, 2.5]]), '#8ad6a6');
  a.path(a.hill(452, [[12, 1.6, 4], [6, 4.2, 1]]), '#4cb68f');
  a.rect(0, 462, 600, 440, a.lin([[0.51, '#a4efe2'], [0.7, '#37c8c9'], [1, '#0b7f9e']]));
  for (let i = 0; i < 46; i++) { const y = 480 + a.r() ** 1.5 * 400; const w = 24 + ((y - 470) / 420) * 120; a.rect(a.r() * 640 - 30, y, w, 2 + ((y - 470) / 420) * 4, '#fff', 0.5, 2); }
  // Sailboat.
  const x = 372; const y = 620;
  a.path(`M${x - 92},${y} L${x + 92},${y} L${x + 62},${y + 42} L${x - 62},${y + 42} Z`, '#f2a93b');
  a.path(`M${x - 4},${y - 8} L${x - 4},${y - 240} L${x - 112},${y - 8} Z`, '#ffffff');
  a.path(`M${x + 6},${y - 8} L${x + 6},${y - 196} L${x + 88},${y - 8} Z`, '#ff6a5c');
  a.path(`M${x - 70},${y + 46} Q${x},${y + 66} ${x + 70},${y + 46} L${x + 70},${y + 90} L${x - 70},${y + 90} Z`, '#0b7f9e', 0.35);
  a.text(['Bright', 'water'], { x: 556, y: 110, size: 100, lead: 0.86, heavy: 1.2, track: -2, fill: '#0a5a74', anchor: 'end' });
  return a.finish({ vignette: 0, grain: 0.05 });
};

// ---------------------------------------------------------------- Ashfall: volcano, lava lines.
scenes['ashfall'] = () => {
  const a = new Art(600, 900, 54);
  a.bg(a.lin([[0, '#0d0c10'], [0.36, '#2a1c1e'], [0.58, '#6c2a1a'], [0.7, '#c9471a'], [1, '#c9471a']]));
  for (let i = 0; i < 26; i++) { const x = 300 + (a.r() - 0.5) * 320 * (1 - i / 40); const y = 430 - i * 20 + a.r() * 40; a.glow(x, y, 80 + i * 8, i < 8 ? '#a1401e' : '#3b3136', 0.5); }
  a.glow(300, 460, 280, '#ff7a2a', 0.9);
  const v = `M-20,900 L150,660 L255,470 L285,452 L320,456 L350,478 L465,680 L620,900 Z`;
  a.path(v, a.lin([[0.5, '#2e1414'], [1, '#0a0507']]));
  const lava = (d, w) => { a.line(d, '#ff6a1a', w + 6, 0.7, a.blur(5)); a.line(d, '#ffb347', w); a.line(d, '#fff3c4', w * 0.35); };
  lava('M300,456 C296,520 322,560 305,620 C292,668 266,700 280,770 C290,820 264,860 268,906', 7);
  lava('M308,620 C344,650 354,700 384,740 C410,776 420,840 446,906', 4);
  lava('M284,700 C244,736 226,790 196,830 C174,860 168,884 162,906', 3.4);
  a.path(a.ridge(856, 44, 0.6), '#060305');
  for (let i = 0; i < 120; i++) { a.circle(a.r() * 600, a.r() * 900, 0.6 + a.r() ** 3 * 2.6, a.r() > 0.82 ? '#ffb347' : '#b8b2ad', 0.2 + a.r() * 0.6); }
  a.text(['ASHFALL'], { x: 300, y: 134, size: 96, anchor: 'middle', track: 3, heavy: 2, fill: '#ece4da' });
  return a.finish({ vignette: 0.35 });
};

// ---------------------------------------------------------------- Deep Field: aurora over a radio dish.
scenes['deep-field'] = () => {
  const a = new Art(600, 900, 71);
  a.bg(a.lin([[0, '#040818'], [0.6, '#0b1a3a'], [1, '#143a4a']]));
  a.stars(320, 720, '#e6f2ff', 1.5);
  const f = a.blur(16);
  const curtain = (y0, amp, ph, col, h, op) => {
    for (let i = 0; i < 70; i++) {
      const x = (i / 70) * 640 - 20; const y = y0 + Math.sin(i * 0.17 + ph) * amp + Math.sin(i * 0.05 + ph * 2) * amp * 1.4;
      const hh = h * (0.6 + 0.4 * Math.sin(i * 0.5 + ph));
      a.add(`<rect x="${x}" y="${y - hh}" width="11" height="${hh}" fill="${a.lin([[(y - hh) / 900, col, 0], [y / 900, col, op]])}" ${f}/>`);
    }
  };
  curtain(430, 44, 0.4, '#6d4dff', 300, 0.5);
  curtain(490, 40, 2.1, '#37e0a8', 320, 0.72);
  curtain(540, 34, 3.8, '#9df2c8', 170, 0.45);
  a.path(a.ridge(712, 60, 0.55), '#0a2233');
  a.path(a.ridge(772, 60, 0.55), '#051421');
  // Dish.
  const cx = 300; const cy = 690; const k = '#02070f';
  a.add(`<g transform="rotate(-28 ${cx} ${cy})"><path d="M${cx - 130},${cy - 34} Q${cx},${cy + 92} ${cx + 130},${cy - 34} Q${cx},${cy + 22} ${cx - 130},${cy - 34} Z" fill="${k}"/><path d="M${cx},${cy + 32} L${cx},${cy - 92}" stroke="${k}" stroke-width="7"/><circle cx="${cx}" cy="${cy - 96}" r="8" fill="#f2a93b"/></g>`);
  a.path(`M${cx - 18},${cy + 40} L${cx - 62},${cy + 180} L${cx + 62},${cy + 180} L${cx + 18},${cy + 40} Z`, k);
  a.path(a.ridge(862, 26, 0.5), k);
  a.text(['DEEP', 'FIELD'], { x: 44, y: 116, size: 74, lead: 1.0, track: 12, weight: 400, fill: '#eafff6' });
  return a.finish({ vignette: 0.25 });
};

// ---------------------------------------------------------------- Night Courier: neon alley in the rain.
scenes['night-courier'] = () => {
  const a = new Art(600, 900, 33);
  a.bg(a.lin([[0, '#07081c'], [0.5, '#1a1050'], [0.74, '#7a1f8c'], [1, '#04050f']]));
  a.glow(300, 620, 360, '#ff3fa4', 0.75);
  a.glow(300, 380, 300, '#27d7ff', 0.28);
  const bld = (x, w, top, col, winC) => {
    a.rect(x, top, w, 900 - top, col);
    for (let wy = top + 16; wy < 730; wy += 22) for (let wx = x + 10; wx < x + w - 14; wx += 18) if (a.r() > 0.55) a.rect(wx, wy, 9, 13, a.r() > 0.75 ? '#ffc765' : winC, 0.25 + a.r() * 0.7);
  };
  bld(190, 110, 200, '#141245', '#6df0ff'); bld(300, 120, 270, '#191456', '#ff7ac6');
  bld(-10, 170, 110, '#0b0b2b', '#27d7ff'); bld(440, 180, 60, '#0c0b30', '#ff5ca8');
  // Signs.
  a.rect(150, 300, 54, 170, '#ff2f96', 1, 6); a.add(`<rect x="146" y="296" width="62" height="178" rx="8" fill="#ff2f96" ${a.blur(12)}/>`);
  a.rect(150, 300, 54, 170, '#ff5cb0', 1, 6);
  a.rect(410, 390, 66, 130, '#23d8ff', 1, 6); a.add(`<rect x="406" y="386" width="74" height="138" rx="8" fill="#23d8ff" ${a.blur(12)}/>`);
  a.rect(410, 390, 66, 130, '#7ceaff', 1, 6);
  // Street reflection.
  a.rect(0, 730, 600, 170, a.lin([[730 / 900, '#2a0f55'], [1, '#04050f']]));
  a.rect(150, 736, 54, 150, '#ff2f96', 0.42); a.rect(410, 736, 66, 130, '#23d8ff', 0.36);
  a.glow(300, 738, 240, '#ff7ac6', 0.42);
  // Courier on a bike, silhouette.
  const x = 300; const y = 720; const k = '#04040e';
  a.add(`<circle cx="${x - 54}" cy="${y}" r="28" fill="none" stroke="${k}" stroke-width="7"/><circle cx="${x + 56}" cy="${y}" r="28" fill="none" stroke="${k}" stroke-width="7"/>`);
  a.path(`M${x - 54},${y} L${x - 8},${y - 44} L${x + 40},${y - 44} L${x + 56},${y} L${x + 30},${y - 66} L${x + 14},${y - 66}`, 'none', 1, `stroke="${k}" stroke-width="7" stroke-linecap="round" stroke-linejoin="round"`);
  a.path(`M${x - 22},${y - 44} L${x - 30},${y - 104} Q${x - 26},${y - 124} ${x},${y - 106} L${x + 26},${y - 76} L${x + 20},${y - 66} L${x - 8},${y - 88} L${x},${y - 44} Z`, k);
  a.circle(x - 16, y - 132, 15, k);
  a.rect(x - 62, y - 118, 42, 42, '#f2a93b', 1, 6);
  // Rain.
  let rain = '';
  for (let i = 0; i < 180; i++) { const rx = a.r() * 660 - 30; const ry = a.r() * 860; const l = 12 + a.r() * 26; rain += `M${rx},${ry} L${rx - l * 0.22},${ry + l} `; }
  a.line(rain, '#cfe6ff', 1.1, 0.34);
  a.text(['NIGHT', 'COURIER'], { x: 44, y: 106, size: 82, lead: 0.92, skew: -8, heavy: 2, track: 0, fill: '#fff' });
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Desktop: the one tile that isn't a game.
scenes['desktop'] = () => {
  const a = new Art(600, 900, 3);
  a.bg(a.lin([[0, '#14161f'], [1, '#232740']]));
  const mx = 300; const my = 330; const mr = 160;
  a.glow(mx, my, 480, '#f2a93b', 0.55);
  a.circle(mx, my, mr, a.lin([[0.2, '#ffc765'], [0.55, '#f2a93b']]));
  for (const r of [230, 300, 370, 440]) a.add(`<circle cx="${mx}" cy="${my}" r="${r}" fill="none" stroke="#f4efe6" stroke-width="1.2" opacity=".12"/>`);
  const win = (x, y, w, h, op) => {
    a.rect(x, y, w, h, '#161a2c', op, 18);
    a.add(`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="18" fill="none" stroke="#fff" stroke-opacity=".28" stroke-width="1.5"/>`);
    for (let i = 0; i < 3; i++) a.circle(x + 22 + i * 18, y + 22, 5, '#fff', 0.42);
    a.rect(x + 18, y + 48, w * 0.55, 10, '#fff', 0.34, 5); a.rect(x + 18, y + 70, w * 0.75, 10, '#fff', 0.18, 5); a.rect(x + 18, y + 92, w * 0.42, 10, '#fff', 0.18, 5);
  };
  win(60, 420, 300, 200, 0.78); win(240, 520, 300, 200, 0.86);
  a.text(['Desktop'], { x: 44, y: 830, size: 92, heavy: 1.2, track: -2, fill: '#f4efe6' });
  return a.finish({ vignette: 0.2 });
};

// ---------------------------------------------------------------- Strategy: a hex map at dusk, two armies.
// A polygon from [x, y] points.
const poly = (pts) => 'M' + pts.map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' L') + ' Z';
// A cloud or a smoke puff.
const puff = (a, x, y, s, c, op = 0.92) => { for (const [dx, dy, r] of [[0, 0, 34], [38, -14, 42], [80, 0, 32], [40, 10, 40]]) a.circle(x + dx * s, y + dy * s, r * s, c, op); };
// A flag on a pole, standing on [x, y].
const flag = (a, x, y, c, k = '#0a0d12') => { a.rect(x - 2, y - 70, 4, 70, k); a.path(poly([[x + 2, y - 70], [x + 44, y - 56], [x + 2, y - 42]]), c); };
scenes['strategy'] = () => {
  const a = new Art(600, 900, 47);
  a.bg(a.lin([[0, '#081620'], [0.5, '#11343c'], [1, '#2b2a1c']]));
  const R = 44; const dx = R * 1.732; const dy = R * 1.5;
  const cols = ['#14566a', '#1f6a4f', '#2f7f55', '#8a9a55', '#c9a45a'];
  for (let row = 0; row < 11; row++) for (let col = -1; col < 9; col++) {
    const x = col * dx + (row % 2 ? dx / 2 : 0); const y = 240 + row * dy;
    a.path(poly([0, 1, 2, 3, 4, 5].map((i) => [x + R * Math.sin(i * Math.PI / 3), y + R * Math.cos(i * Math.PI / 3)])), cols[Math.floor(a.r() * a.r() * 5)], 1, 'stroke="#081620" stroke-width="3"');
  }
  a.glow(300, 500, 330, '#ffc765', 0.5);
  a.line('M150,640 C230,560 300,560 400,420', '#ffc765', 7, 1, 'stroke-dasharray="4 20"');
  a.path(poly([[400, 420], [370, 430], [392, 456]]), '#ffc765');
  flag(a, 130, 660, '#58b7ff'); flag(a, 196, 700, '#58b7ff'); flag(a, 90, 720, '#58b7ff');
  flag(a, 420, 410, '#ff4a5e'); flag(a, 484, 370, '#ff4a5e'); flag(a, 500, 450, '#ff4a5e');
  a.bg(a.lin([[0.14, '#081620', 1], [0.3, '#081620', 0]]));
  a.text(['STRATEGY'], { x: 300, y: 128, size: 88, anchor: 'middle', track: 5, heavy: 1.5, fill: '#fff4dc' });
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Skybound: a glider above the clouds.
scenes['skybound'] = () => {
  const a = new Art(600, 900, 28);
  a.bg(a.lin([[0, '#123a8a'], [0.4, '#3f7fd0'], [0.66, '#f5b7b0'], [1, '#ffdca8']]));
  a.glow(150, 560, 340, '#fff3cf', 0.95);
  a.circle(150, 560, 58, '#fffbe8');
  a.line('M-20,640 C150,590 300,470 410,350', '#ffffff', 5, 0.75);
  const x = 414; const y = 346;
  a.path(poly([[x + 74, y - 58], [x + 10, y + 20], [x - 20, y + 12], [x - 2, y - 14]]), '#f4f7ff');
  a.path(poly([[x - 90, y - 44], [x + 112, y + 2], [x + 30, y + 12], [x - 10, y - 4]]), '#ffffff');
  a.path(poly([[x + 112, y + 2], [x + 30, y + 12], [x - 90, y - 44], [x, y + 30]]), '#a9bfe8', 0.8);
  a.path(poly([[x - 26, y + 8], [x - 58, y - 28], [x - 44, y + 14]]), '#ff6a5c');
  for (const [cx, cy, s, c] of [[420, 520, 1.4, '#ffe4d6'], [10, 640, 1.8, '#ffeadb'], [320, 650, 2, '#fff2e6'], [-30, 740, 2.6, '#fff8ef'], [260, 770, 3, '#ffffff']]) puff(a, cx, cy, s, c, 1);
  a.text(['Skybound'], { x: 300, y: 146, size: 112, anchor: 'middle', heavy: 1.5, track: -2, fill: '#ffffff' });
  return a.finish({ vignette: 0.14, grain: 0.08 });
};

// ---------------------------------------------------------------- Cinder Road: a bike on a burnt highway.
scenes['cinder-road'] = () => {
  const a = new Art(600, 900, 61);
  a.bg(a.lin([[0, '#100a0c'], [0.3, '#35151a'], [0.5, '#a33a18'], [0.6, '#ff8d3a'], [1, '#ff8d3a']]));
  a.glow(300, 520, 330, '#ffb55c', 0.9);
  a.path(a.ridge(510, 60, 0.6), '#4a1a14');
  a.path(a.ridge(540, 34, 0.55), '#1c0b0d');
  a.path(`M0,548 L600,548 L600,900 L0,900 Z`, a.lin([[0.6, '#1c0b0d'], [1, '#050304']]));
  a.path(poly([[290, 548], [310, 548], [560, 900], [40, 900]]), a.lin([[0.6, '#5a2a22'], [1, '#17090b']]));
  for (let i = 0; i < 9; i++) { const t = (i / 9) ** 1.8; const y = 552 + t * 340; const hw = 1 + t * 7; a.path(poly([[300 - hw, y], [300 + hw, y], [300 + hw * 1.3, y + 8 + t * 40], [300 - hw * 1.3, y + 8 + t * 40]]), '#ffcf8f', 0.9); }
  // The rider from behind.
  const k = '#050304';
  a.path('M270,740 C268,690 278,664 300,658 C322,664 332,690 330,740 L322,784 L278,784 Z', k);
  a.circle(300, 642, 22, k);
  a.rect(290, 780, 20, 44, k, 1, 6);
  a.add(`<rect x="280" y="736" width="40" height="10" rx="4" fill="#ff2b3a" ${a.blur(6)}/>`);
  a.rect(284, 738, 32, 6, '#ffb0a8', 1, 3);
  for (let i = 0; i < 110; i++) a.circle(a.r() * 600, a.r() * 760, 0.7 + a.r() ** 2 * 2.6, a.r() > 0.4 ? '#ffb347' : '#ff6a2a', 0.35 + a.r() * 0.6);
  a.text(['CINDER', 'ROAD'], { x: 300, y: 130, size: 112, lead: 0.92, anchor: 'middle', skew: -6, heavy: 2, fill: '#fff0dc' });
  return a.finish({ vignette: 0.32 });
};

// ---------------------------------------------------------------- Glass Harbor: towers and their reflection.
scenes['glass-harbor'] = () => {
  const a = new Art(600, 900, 72);
  a.bg(a.lin([[0, '#051a2c'], [0.42, '#0d5a78'], [0.6, '#7fe4dc'], [1, '#04202f']]));
  a.glow(320, 520, 280, '#c8fff0', 0.85);
  let x = -10;
  while (x < 610) {
    const w = 44 + a.r() * 60; const h = 90 + a.r() * 190; const c = a.lin([[(540 - h) / 900, '#bff3ee', 0.95], [0.6, '#135a78', 0.9]]);
    a.rect(x, 540 - h, w, h, c);
    for (let y = 540 - h + 10; y < 530; y += 18) a.rect(x + 3, y, w - 6, 2, '#04202f', 0.3);
    a.rect(x, 544, w, h * 0.8, c, 0.28);
    x += w + 3;
  }
  a.rect(0, 538, 600, 3, '#eafffa', 0.9);
  for (let i = 0; i < 44; i++) { const y = 548 + a.r() ** 1.4 * 330; a.rect(a.r() * 620 - 20, y, 30 + a.r() * 90, 1.6, '#d6fff6', 0.34); }
  // A small ferry.
  a.path(poly([[230, 640], [370, 640], [346, 668], [254, 668]]), '#04121d');
  a.rect(262, 616, 76, 24, '#04121d', 1, 3);
  for (let i = 0; i < 5; i++) a.rect(270 + i * 13, 623, 8, 8, '#ffc765');
  a.text(['Glass', 'Harbor'], { x: 44, y: 128, size: 108, lead: 0.9, heavy: 1.2, track: -3, fill: '#f0fffb' });
  return a.finish({ vignette: 0.24 });
};

// ---------------------------------------------------------------- Hollow Peak: one mountain with a lit cave.
scenes['hollow-peak'] = () => {
  const a = new Art(600, 900, 83);
  a.bg(a.lin([[0, '#0a0a26'], [0.45, '#2a2468'], [0.7, '#6f4ea6'], [1, '#1a1440']]));
  a.stars(190, 420, '#e8e6ff', 1.5);
  a.circle(120, 330, 46, '#f1ecff');
  a.circle(138, 322, 42, '#2a2468');
  a.path(poly([[-40, 900], [300, 260], [640, 900]]), a.lin([[0.3, '#463c86'], [0.9, '#120e2e']]));
  a.path(poly([[300, 260], [640, 900], [420, 900], [340, 520]]), '#0e0a24', 0.6);
  a.path(poly([[300, 260], [372, 396], [340, 372], [316, 416], [292, 370], [262, 410], [234, 384]]), '#f1f0ff');
  a.glow(300, 690, 170, '#ffb347', 0.95);
  a.path('M258,780 C252,700 276,646 300,646 C324,646 348,700 342,780 Z', '#ffcf7a');
  a.path('M280,780 C276,724 290,690 300,690 C310,690 324,724 320,780 Z', '#fff3d0');
  a.path(a.ridge(800, 40, 0.55), '#07051a');
  a.text(['HOLLOW', 'PEAK'], { x: 300, y: 112, size: 92, lead: 0.98, anchor: 'middle', track: 7, weight: 400, heavy: 1.5, fill: '#f4f1ff' });
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Iron Tide: an ironclad in heavy seas.
scenes['iron-tide'] = () => {
  const a = new Art(600, 900, 38);
  a.bg(a.lin([[0, '#1c232b'], [0.35, '#46505a'], [0.52, '#d6743a'], [0.62, '#ffb169'], [1, '#ffb169']]));
  a.glow(430, 520, 250, '#ffd9a0', 0.8);
  for (let i = 0; i < 12; i++) puff(a, a.r() * 640 - 60, 210 + a.r() * 190, 1.4 + a.r() * 1.6, '#1c232b', 0.4);
  // The ship.
  const k = '#11161c';
  a.path(poly([[70, 590], [530, 590], [490, 660], [120, 660]]), k);
  a.path(poly([[160, 590], [160, 550], [420, 550], [440, 590]]), k);
  a.rect(240, 480, 90, 72, k); a.rect(262, 420, 22, 62, k); a.rect(300, 440, 18, 42, k);
  a.rect(370, 524, 110, 9, k); a.rect(100, 540, 84, 8, k);
  for (let i = 0; i < 12; i++) puff(a, 262 + i * 18 + a.r() * 16, 408 - i * 15, 0.3 + i * 0.1, '#20262e', 0.72);
  for (let i = 0; i < 6; i++) a.circle(182 + i * 42, 572, 4.5, '#ffc765');
  // Waves.
  [[640, '#31414f', 16], [684, '#222f3b', 22], [744, '#141e28', 28], [812, '#0a1219', 34]].forEach(([b, c, amp], i) => {
    a.path(a.hill(b, [[amp, 3 + i, i * 1.3], [amp * 0.4, 8 + i, i * 2.1]]), c);
  });
  a.text(['IRON', 'TIDE'], { x: 44, y: 130, size: 122, lead: 0.92, heavy: 3, track: 2, fill: '#f6efe6' });
  return a.finish({ vignette: 0.34 });
};

// ---------------------------------------------------------------- Jade Circuit: traces running to one chip.
scenes['jade-circuit'] = () => {
  const a = new Art(600, 900, 90);
  a.bg(a.lin([[0, '#02140f'], [0.6, '#053226'], [1, '#02140f']]));
  a.glow(300, 500, 380, '#25d898', 0.5);
  for (let i = 0; i < 46; i++) {
    let x = 20 + (i % 23) * 25; let y = i < 23 ? 880 : 240; const dir = i < 23 ? -1 : 1;
    let d = `M${x},${y}`;
    const stop = 500 + dir * (70 + a.r() * 40);
    while ((stop - y) * dir > 0) {
      y += dir * (28 + a.r() * 40);
      if ((stop - y) * dir < 0) y = stop;
      d += ` L${x},${y}`;
      const nx = x + (300 - x) * (0.2 + a.r() * 0.3);
      y += dir * Math.abs(nx - x) * 0.6;
      x = nx;
      d += ` L${x.toFixed(1)},${y.toFixed(1)}`;
    }
    a.line(d, i % 5 ? '#29c98f' : '#a7ffd9', i % 5 ? 2.4 : 3.4, 0.8);
    a.circle(x, y, 5, '#a7ffd9');
  }
  a.rect(190, 390, 220, 220, '#02140f', 1, 18);
  a.add(`<rect x="190" y="390" width="220" height="220" rx="18" fill="none" stroke="#a7ffd9" stroke-width="4"/>`);
  a.glow(300, 500, 90, '#a7ffd9', 0.9);
  a.path(poly([[300, 442], [346, 500], [300, 558], [254, 500]]), '#eafff6');
  a.text(['JADE', 'CIRCUIT'], { x: 300, y: 112, size: 88, lead: 0.98, anchor: 'middle', track: 6, heavy: 1.2, fill: '#eafff6' });
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Kite Season: bright hill, high kites.
scenes['kite-season'] = () => {
  const a = new Art(600, 900, 14);
  a.bg(a.lin([[0, '#49b4ea'], [0.55, '#c6ecf4'], [0.75, '#fff3cd'], [1, '#fff3cd']]));
  a.glow(480, 300, 240, '#fffbe2', 0.95);
  for (const [x, y, s] of [[40, 340, 0.8], [360, 500, 0.7], [90, 560, 0.6]]) puff(a, x, y, s, '#fff', 0.9);
  const kite = (x, y, s, c1, c2, tx, ty) => {
    a.line(`M${x},${y + 64 * s} Q${x + (tx - x) * 0.3},${y + 220 * s} ${tx},${ty}`, '#26485c', 1.8, 0.7);
    a.path(poly([[x, y - 70 * s], [x + 50 * s, y], [x, y + 64 * s]]), c1);
    a.path(poly([[x, y - 70 * s], [x - 50 * s, y], [x, y + 64 * s]]), c2);
    for (let i = 1; i < 4; i++) a.path(poly([[x + 10 * s, y + (64 + i * 28) * s], [x, y + (76 + i * 28) * s], [x - 10 * s, y + (64 + i * 28) * s], [x, y + (52 + i * 28) * s]]), c1);
  };
  kite(170, 410, 1.5, '#ff5c6a', '#ffc24a', 300, 690);
  kite(430, 340, 1.1, '#6a4dff', '#3fd0c8', 310, 690);
  kite(320, 530, 0.7, '#ff8a3a', '#ffe36b', 305, 690);
  a.path(a.hill(742, [[36, 0.8, 1.4], [10, 2.4, 0.4]]), '#7ec85c');
  a.path(a.hill(796, [[26, 1.1, 3.4], [8, 3, 2]]), '#4ca34c');
  a.rect(298, 660, 12, 40, '#23324a', 1, 5); a.circle(304, 650, 12, '#23324a');
  a.text(['Kite', 'Season'], { x: 44, y: 124, size: 104, lead: 0.92, heavy: 1.2, track: -3, fill: '#0c3a5c' });
  return a.finish({ vignette: 0, grain: 0.05 });
};

// ---------------------------------------------------------------- Lantern Row: an alley strung with lights.
scenes['lantern-row'] = () => {
  const a = new Art(600, 900, 26);
  a.bg(a.lin([[0, '#140614'], [0.5, '#4a0f22'], [0.75, '#a32d1c'], [1, '#150608']]));
  a.glow(300, 520, 300, '#ff9e45', 0.75);
  a.path(poly([[0, 170], [240, 430], [240, 650], [0, 900]]), '#120410');
  a.path(poly([[600, 170], [360, 430], [360, 650], [600, 900]]), '#0d030c');
  a.path(poly([[0, 900], [240, 650], [360, 650], [600, 900]]), a.lin([[0.72, '#6b2420'], [1, '#120508']]));
  [[-1, 1], [1, 0.72], [-1, 0.5], [1, 0.34], [-1, 0.22], [1, 0.14]].forEach(([side, s], i) => {
    const x = 300 + side * (28 + s * 210); const y = 500 - s * 200;
    a.line(`M${x - side * s * 150},${y - s * 100} Q${x},${y - s * 30} ${x + side * s * 70},${y - s * 90}`, '#120410', 1.5 + s * 2);
    a.glow(x, y + s * 40, 130 * s + 20, '#ff9e45', 0.75);
    a.add(`<ellipse cx="${x}" cy="${y + s * 40}" rx="${44 * s}" ry="${56 * s}" fill="${i % 2 ? '#ffb347' : '#ff5c3a'}"/>`);
    a.add(`<ellipse cx="${x}" cy="${y + s * 40}" rx="${18 * s}" ry="${50 * s}" fill="#ffe2a0" opacity=".65"/>`);
    a.rect(x - 16 * s, y - 20 * s, 32 * s, 8 * s, '#2a0a12'); a.rect(x - 16 * s, y + 92 * s, 32 * s, 8 * s, '#2a0a12');
  });
  a.path('M286,690 C284,650 290,630 300,628 C310,630 316,650 314,690 L312,740 L288,740 Z', '#0a0308');
  a.circle(300, 616, 11, '#0a0308');
  a.text(['Lantern', 'Row'], { x: 300, y: 112, size: 104, lead: 0.92, anchor: 'middle', heavy: 1.2, track: -2, fill: '#fff0d6' });
  return a.finish({ vignette: 0.32 });
};

// ---------------------------------------------------------------- Moss and Stone: standing stones in mist.
scenes['moss-and-stone'] = () => {
  const a = new Art(600, 900, 57);
  a.bg(a.lin([[0, '#0d2a22'], [0.4, '#3a7a5c'], [0.6, '#cfe8b8'], [1, '#0d2a22']]));
  a.glow(300, 470, 300, '#f6ffd8', 0.9);
  for (const [b, c, amp] of [[470, '#5f9a70', 70], [520, '#3a7a5c', 60]]) {
    a.path(a.ridge(b, amp, 0.6), c);
    const rd = a.lastRidge; let d = '';
    for (let i = 0; i < 64; i++) { const p = rd[Math.floor(a.r() * rd.length)]; const h = 26 + a.r() * 48; d += `M${p[0]},${p[1] - h} L${p[0] + h * 0.22},${p[1] + 4} L${p[0] - h * 0.22},${p[1] + 4} Z `; }
    a.path(d, c);
  }
  a.path(a.hill(640, [[22, 0.9, 1], [8, 2.6, 2]]), '#1f5a3c');
  const stone = (x, top, w) => {
    const b = 720 + (a.r() - 0.5) * 30; const pts = [[x - w / 2, b], [x - w * 0.42, top + 26], [x - w * 0.1, top], [x + w * 0.36, top + 14], [x + w / 2, b]];
    a.path(poly(pts), a.lin([[top / 900, '#8d9a92'], [b / 900, '#3a4840']], (x - w) / 600, 0, (x + w) / 600, 0));
    a.path(poly([[x - w / 2, b], [x - w * 0.42, top + 26], [x - w * 0.1, top], [x - w * 0.04, top + 40], [x - w * 0.24, b - 70], [x, b]]), '#5fae4f', 0.92);
  };
  stone(120, 480, 92); stone(238, 420, 110); stone(372, 450, 100); stone(490, 500, 88);
  a.path(a.hill(742, [[14, 1.1, 3.2], [8, 3.2, 1.2]]), '#0e3a24');
  a.text(['Moss and', 'Stone'], { x: 44, y: 118, size: 92, lead: 0.96, heavy: 1.2, track: -2, fill: '#f6ffe8' });
  return a.finish({ vignette: 0.28 });
};

// ---------------------------------------------------------------- Orbit Garden: a dome of green above a planet.
scenes['orbit-garden'] = () => {
  const a = new Art(600, 900, 44);
  a.bg(a.lin([[0, '#040818'], [0.7, '#0c1c44'], [1, '#123a58']]));
  a.stars(300, 760, '#e6f2ff', 1.5);
  a.circle(300, 1250, 620, a.rad(300, 1250, 620, [[0.85, '#1e6aa8'], [0.97, '#58c6e8'], [1, '#c6f4ff']]));
  // The dome.
  const cx = 300; const cy = 610; const R = 210;
  a.glow(cx, cy - 70, 300, '#6df0a0', 0.55);
  const id = a.id(); a.defs.push(`<clipPath id="${id}"><path d="M${cx - R},${cy} A${R},${R} 0 0 1 ${cx + R},${cy} Z"/></clipPath>`);
  let plants = `<rect x="${cx - R}" y="${cy - 40}" width="${R * 2}" height="40" fill="#1a5a34"/>`;
  for (let i = 0; i < 26; i++) { const x = cx - R + a.r() * R * 2; const h = 40 + a.r() * 130; const c = ['#2f9a58', '#49c06a', '#8ae07a', '#ff8fb0'][Math.floor(a.r() * 4)]; plants += `<path d="M${x},${cy} Q${x + 6},${cy - h * 0.6} ${x},${cy - h}" stroke="#1a5a34" stroke-width="4" fill="none"/><circle cx="${x}" cy="${cy - h}" r="${12 + a.r() * 22}" fill="${c}"/>`; }
  a.add(`<g clip-path="url(#${id})"><path d="M${cx - R},${cy} A${R},${R} 0 0 1 ${cx + R},${cy} Z" fill="#0d2c34"/>${plants}</g>`);
  a.add(`<path d="M${cx - R},${cy} A${R},${R} 0 0 1 ${cx + R},${cy}" fill="none" stroke="#d6fff4" stroke-width="4" opacity=".85"/>`);
  for (const q of [-0.5, 0, 0.5]) a.add(`<path d="M${cx + q * R},${cy} Q${cx + q * R * 0.6},${cy - R * 0.9} ${cx},${cy - R}" fill="none" stroke="#d6fff4" stroke-width="1.5" opacity=".4"/>`);
  a.add(`<path d="M${cx - 170},${cy - 150} A${R - 18},${R - 18} 0 0 1 ${cx - 60},${cy - 192}" fill="none" stroke="#fff" stroke-width="7" stroke-linecap="round" opacity=".5"/>`);
  a.rect(cx - R - 20, cy, R * 2 + 40, 22, '#c9d8e6', 1, 6);
  a.rect(cx - 90, cy + 22, 180, 34, '#6c7f96', 1, 8);
  a.path(poly([[cx - R - 20, cy + 10], [cx - R - 130, cy - 6], [cx - R - 130, cy + 32]]), '#f2a93b');
  a.path(poly([[cx + R + 20, cy + 10], [cx + R + 130, cy - 6], [cx + R + 130, cy + 32]]), '#f2a93b');
  a.text(['Orbit', 'Garden'], { x: 44, y: 124, size: 108, lead: 0.92, heavy: 1.2, track: -3, fill: '#eafff4' });
  return a.finish({ vignette: 0.22 });
};

// ---------------------------------------------------------------- Paper Fleet: folded boats on a paper sea.
scenes['paper-fleet'] = () => {
  const a = new Art(600, 900, 67);
  a.bg(a.lin([[0, '#f6ead0'], [0.4, '#f2cf9e'], [0.5, '#6fb7cf'], [1, '#17557c']]));
  a.glow(450, 290, 200, '#fff8e2', 1);
  a.circle(450, 290, 50, '#fff');
  // Folded waves, flat facets.
  for (let row = 0; row < 8; row++) {
    const y = 440 + row * 60; const w = 62 + row * 12; const c = ['#7fc4d8', '#5fa9c6', '#3f8eb2', '#2b7aa0'][row % 4]; const c2 = ['#a4dbe6', '#7fc4d8', '#5fa9c6', '#3f8eb2'][row % 4];
    for (let x = -w + (row % 2) * (w / 2); x < 620; x += w) { a.path(poly([[x, y + 60], [x + w / 2, y], [x + w / 2, y + 60]]), c); a.path(poly([[x + w, y + 60], [x + w / 2, y], [x + w / 2, y + 60]]), c2); }
  }
  const boat = (x, y, s, c) => {
    a.path(poly([[x - 90 * s, y], [x + 90 * s, y], [x + 50 * s, y + 52 * s], [x - 50 * s, y + 52 * s]]), c);
    a.path(poly([[x - 90 * s, y], [x, y + 26 * s], [x + 90 * s, y], [x + 50 * s, y + 52 * s], [x - 50 * s, y + 52 * s]]), '#0e2c44', 0.14);
    a.path(poly([[x - 46 * s, y], [x, y - 92 * s], [x, y + 26 * s]]), '#fffdf6');
    a.path(poly([[x + 46 * s, y], [x, y - 92 * s], [x, y + 26 * s]]), '#d8cdb4');
  };
  boat(150, 500, 0.5, '#fff4dc'); boat(460, 520, 0.6, '#ffd9a0');
  boat(300, 600, 1.5, '#ff6a5c');
  boat(110, 720, 0.8, '#fff4dc'); boat(490, 730, 0.9, '#fff4dc');
  a.text(['Paper', 'Fleet'], { x: 44, y: 124, size: 112, lead: 0.92, heavy: 1.2, track: -3, fill: '#17405e' });
  return a.finish({ vignette: 0, grain: 0.06 });
};

// ---------------------------------------------------------------- Quiet Engine: a turbine, barely lit.
scenes['quiet-engine'] = () => {
  const a = new Art(600, 900, 11);
  a.bg(a.lin([[0, '#0c0f14'], [0.7, '#1b212b'], [1, '#0a0c10']]));
  const cx = 300; const cy = 510;
  a.glow(cx, cy, 340, '#f2a93b', 0.4);
  for (const [r, sw, op] of [[258, 26, 0.9], [226, 2, 0.5], [196, 2, 0.4]]) a.add(`<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="#3d4654" stroke-width="${sw}" opacity="${op}"/>`);
  for (let i = 0; i < 24; i++) { const t = (i / 24) * Math.PI * 2; a.circle(cx + Math.cos(t) * 258, cy + Math.sin(t) * 258, 5, '#7c8798'); }
  for (let i = 0; i < 18; i++) {
    const t = (i / 18) * Math.PI * 2; const t2 = t + 0.5; const t3 = t + 0.18;
    a.path(poly([[cx + Math.cos(t) * 52, cy + Math.sin(t) * 52], [cx + Math.cos(t2) * 236, cy + Math.sin(t2) * 236], [cx + Math.cos(t3) * 236, cy + Math.sin(t3) * 236]]), i % 2 ? '#39424f' : '#2b333e', 1, 'stroke="#0c0f14" stroke-width="2"');
  }
  a.circle(cx, cy, 58, a.rad(cx, cy, 58, [[0, '#fff0c2'], [0.55, '#f2a93b'], [1, '#7a4a10']]));
  a.circle(cx, cy, 15, '#0c0f14');
  a.text(['QUIET', 'ENGINE'], { x: 44, y: 106, size: 72, lead: 1, track: 10, weight: 400, fill: '#f4efe6' });
  a.rect(44, 186, 72, 3, '#f2a93b');
  return a.finish({ vignette: 0.3 });
};

// ---------------------------------------------------------------- Red Mesa: desert buttes at noon's end.
scenes['red-mesa'] = () => {
  const a = new Art(600, 900, 52);
  a.bg(a.lin([[0, '#2a3f7a'], [0.34, '#c8688a'], [0.5, '#ff9f5c'], [0.58, '#ffd29a'], [1, '#ffd29a']]));
  a.glow(180, 430, 260, '#fff0c0', 0.95);
  a.circle(180, 430, 58, '#fff6d6');
  const mesa = (x, w, top, base, c, c2) => {
    a.path(poly([[x - w / 2 - 44, base], [x - w / 2 + 12, top + 40], [x - w / 2 + 4, top], [x + w / 2 - 4, top], [x + w / 2 - 12, top + 40], [x + w / 2 + 44, base]]), c);
    a.path(poly([[x + w * 0.1, top], [x + w / 2 - 4, top], [x + w / 2 - 12, top + 40], [x + w / 2 + 44, base], [x + w * 0.2, base]]), c2);
    for (let i = 1; i < 6; i++) a.rect(x - w / 2 + 2 - i * 5, top + i * ((base - top) / 6), w + i * 10 - 4, 3, '#3a0f12', 0.18);
  };
  mesa(470, 170, 400, 600, '#a83a24', '#6a2018');
  mesa(250, 120, 470, 600, '#8c2c20', '#561a16');
  a.path(a.hill(596, [[8, 1.2, 1], [3, 4, 2]]), '#c9552c');
  a.path(a.hill(660, [[14, 0.9, 2.2], [5, 3, 1]]), '#8f3320');
  a.path(a.hill(750, [[18, 0.7, 4.2], [6, 2.6, 0.5]]), '#4f1814');
  const k = '#16070a';
  a.rect(94, 640, 18, 130, k, 1, 8); a.rect(60, 684, 14, 44, k, 1, 7); a.rect(60, 716, 40, 13, k, 1, 6); a.rect(132, 664, 14, 44, k, 1, 7); a.rect(106, 696, 40, 13, k, 1, 6);
  a.text(['RED', 'MESA'], { x: 44, y: 134, size: 122, lead: 0.92, heavy: 3, track: 3, fill: '#fff4e2' });
  return a.finish({ vignette: 0.24 });
};

// ---------------------------------------------------------------- Salt Flats Rally: a car and its dust, high key.
scenes['salt-flats-rally'] = () => {
  const a = new Art(600, 900, 21);
  a.bg(a.lin([[0, '#2d7dc8'], [0.44, '#bfe6f5'], [0.5, '#ffffff'], [1, '#d8e6ee']]));
  a.path(a.ridge(444, 22, 0.6), '#7f98b8');
  a.rect(0, 452, 600, 450, a.lin([[0.5, '#fbfdff'], [1, '#c6d6e2']]));
  // Salt cracks.
  let d = '';
  for (let i = 0; i < 90; i++) { const y = 470 + a.r() ** 1.5 * 430; const s = 12 + ((y - 460) / 440) * 60; const x = a.r() * 620; d += `M${x},${y} l${s},${s * 0.12} l${s * 0.5},${-s * 0.18} `; }
  a.line(d, '#94aabc', 1.5, 0.55);
  // Dust behind the car.
  for (let i = 0; i < 26; i++) { const t = i / 26; a.circle(420 + t * 230 + a.r() * 30, 640 - t * 160 + (a.r() - 0.5) * (30 + t * 120), 26 + t * 70, '#efe3cf', 0.42); }
  // The car, side on.
  const x = 250; const y = 650;
  a.add(`<ellipse cx="${x}" cy="${y + 54}" rx="170" ry="11" fill="#2a3a4a" opacity=".32"/>`);
  a.path(`M${x - 158},${y + 32} C${x - 160},${y - 2} ${x - 124},${y - 8} ${x - 84},${y - 12} L${x - 44},${y - 58} C${x},${y - 72} ${x + 54},${y - 66} ${x + 80},${y - 18} C${x + 120},${y - 10} ${x + 156},${y - 6} ${x + 160},${y + 32} Z`, '#e4392e');
  a.path(`M${x - 66},${y - 14} L${x - 34},${y - 50} C${x - 8},${y - 58} ${x + 18},${y - 58} ${x + 18},${y - 14} Z M${x + 30},${y - 14} L${x + 30},${y - 56} C${x + 46},${y - 54} ${x + 56},${y - 40} ${x + 66},${y - 14} Z`, '#142436');
  a.rect(x - 158, y + 8, 318, 9, '#fff6ea');
  a.circle(x, y + 12, 26, '#fff6ea'); a.circle(x, y + 12, 22, '#142436');
  for (const wx of [x - 96, x + 104]) { a.circle(wx, y + 32, 28, '#0b131c'); a.circle(wx, y + 32, 12, '#d8e6ee'); }
  a.text(['SALT FLATS', 'RALLY'], { x: 40, y: 104, size: 82, lead: 1.02, skew: -8, heavy: 2, track: -1, fill: '#ffffff' });
  return a.finish({ vignette: 0, grain: 0.05 });
};

// ---------------------------------------------------------------- Tidebreaker: a lighthouse under a great wave.
scenes['tidebreaker'] = () => {
  const a = new Art(600, 900, 96);
  a.bg(a.lin([[0, '#061c2c'], [0.5, '#135a72'], [1, '#02101a']]));
  a.glow(160, 390, 260, '#fff2b5', 0.55);
  // The lighthouse's beam and tower.
  a.path(poly([[160, 360], [640, 240], [640, 420]]), a.lin([[0, '#fff2b5', 0.55], [1, '#fff2b5', 0]], 0.26, 0, 1, 0));
  a.path(poly([[136, 640], [146, 372], [174, 372], [184, 640]]), '#f4f0e6');
  for (const y of [430, 520, 600]) a.path(poly([[140 - (y - 430) * 0.02, y], [180 + (y - 430) * 0.02, y], [181 + (y - 430) * 0.02, y + 36], [139 - (y - 430) * 0.02, y + 36]]), '#e4392e');
  a.rect(140, 346, 40, 26, '#fff2b5'); a.rect(134, 340, 52, 7, '#0a1e2a'); a.path(poly([[140, 340], [160, 316], [180, 340]]), '#0a1e2a');
  a.path(poly([[60, 900], [100, 640], [220, 630], [280, 900]]), '#050f16');
  // The wave curling over from the right.
  a.path('M640,900 L640,190 C520,180 400,230 360,330 C340,380 380,420 420,400 C390,450 330,450 300,420 C250,560 180,740 80,900 Z', a.lin([[0.22, '#7fe0dc'], [0.5, '#1a7a94'], [1, '#04202f']], 0.9, 0, 0.3, 1));
  a.path('M640,190 C520,180 400,230 360,330 C340,380 380,420 420,400 C380,410 356,384 376,340 C410,262 520,226 640,226 Z', '#f0fffb');
  let foam = '';
  for (let i = 0; i < 70; i++) { const t = a.r(); foam += `<circle cx="${(330 + t * 90 + a.r() * 40).toFixed(1)}" cy="${(330 + t * 60 + a.r() * 110).toFixed(1)}" r="${(1 + a.r() * 5).toFixed(1)}" fill="#f0fffb" opacity="${(0.3 + a.r() * 0.6).toFixed(2)}"/>`; }
  a.add(foam);
  a.path(a.hill(832, [[20, 2, 1], [8, 5, 2]]), '#020b12');
  a.text(['TIDEBREAKER'], { x: 300, y: 118, size: 80, anchor: 'middle', heavy: 2, track: 1, fill: '#f0fffb' });
  return a.finish({ vignette: 0.28 });
};

// ---------------------------------------------------------------- Under Ice: a diver's light beneath the sheet.
scenes['under-ice'] = () => {
  const a = new Art(600, 900, 35);
  a.bg(a.lin([[0, '#def8ff'], [0.26, '#6ecdea'], [0.55, '#14609a'], [1, '#02132c']]));
  // The ice sheet's underside.
  for (let i = 0; i < 16; i++) { const x = i * 42 - 10; a.path(poly([[x, 180], [x + 44, 180], [x + 26 + a.r() * 16, 220 + a.r() * 90], [x + 8, 200 + a.r() * 50]]), '#eafcff', 0.9); }
  let cracks = '';
  for (let i = 0; i < 14; i++) { let x = a.r() * 600; let y = 190; cracks += `M${x},${y}`; for (let k = 0; k < 4; k++) { x += (a.r() - 0.5) * 80; y += 20 + a.r() * 26; cracks += ` L${x.toFixed(1)},${y.toFixed(1)}`; } }
  a.line(cracks, '#2b84b0', 2, 0.55);
  for (let i = 0; i < 6; i++) { const x = 60 + i * 96 + a.r() * 30; a.path(poly([[x, 260], [x + 40, 260], [x + 150, 800], [x - 60, 800]]), a.lin([[0.28, '#eafcff', 0.3], [0.86, '#eafcff', 0]])); }
  // The diver and the beam.
  const x = 300; const y = 590; const k = '#03192e';
  a.path(poly([[x + 30, y - 12], [x + 300, y - 130], [x + 300, y + 110]]), a.lin([[0, '#fff2b5', 0.85], [1, '#fff2b5', 0]], 0.55, 0, 1, 0));
  a.add(`<ellipse cx="${x - 30}" cy="${y}" rx="76" ry="24" fill="${k}" transform="rotate(-12 ${x - 30} ${y})"/>`);
  a.circle(x + 44, y - 18, 21, k); a.circle(x + 50, y - 19, 10, '#ffc765');
  a.rect(x - 86, y - 40, 74, 18, '#f2a93b', 1, 9);
  a.path(poly([[x - 92, y + 12], [x - 186, y + 20], [x - 202, y + 52], [x - 92, y + 32]]), k);
  a.circle(x + 58, y - 56, 7, '#d6fff4', 0.8); a.circle(x + 70, y - 88, 5, '#d6fff4', 0.7); a.circle(x + 60, y - 120, 3.5, '#d6fff4', 0.6);
  a.path(a.ridge(800, 50, 0.6), '#01091a');
  a.text(['Under', 'Ice'], { x: 44, y: 88, size: 76, lead: 0.98, heavy: 1.2, track: -1, fill: '#0a3a62' });
  return a.finish({ vignette: 0.18, grain: 0.08 });
};

// ---------------------------------------------------------------- Vector Bloom: a flower from straight lines.
scenes['vector-bloom'] = () => {
  const a = new Art(600, 900, 77);
  a.bg(a.lin([[0, '#100826'], [1, '#2a0c44']]));
  const cx = 300; const cy = 500;
  a.glow(cx, cy, 340, '#ff4fa8', 0.5);
  for (const [n, r0, r1, c1, c2] of [[12, 50, 260, '#ff4fa8', '#7d4dff'], [12, 40, 190, '#ffb14a', '#ff4fa8'], [8, 20, 110, '#fff0c2', '#ffb14a']]) {
    for (let i = 0; i < n; i++) {
      const t = (i / n) * Math.PI * 2 + (n === 12 && r1 === 190 ? Math.PI / 12 : 0); const s = (Math.PI / n) * 0.85;
      const p = (ang, r) => [cx + Math.cos(ang) * r, cy + Math.sin(ang) * r];
      a.path(poly([p(t, r0), p(t + s, r1 * 0.72), p(t, r1)]), c1, 0.92, 'stroke="#100826" stroke-width="1.5"');
      a.path(poly([p(t, r0), p(t - s, r1 * 0.72), p(t, r1)]), c2, 0.92, 'stroke="#100826" stroke-width="1.5"');
    }
  }
  a.circle(cx, cy, 16, '#100826');
  a.line(`M${cx},${cy + 260} L${cx},900 M${cx},${cy + 330} L${cx + 80},${cy + 270} M${cx},${cy + 370} L${cx - 90},${cy + 310}`, '#37e0a8', 5);
  a.text(['VECTOR', 'BLOOM'], { x: 300, y: 112, size: 84, lead: 1, anchor: 'middle', track: 10, weight: 400, heavy: 1.2, fill: '#fdf2ff' });
  return a.finish({ vignette: 0.26 });
};

// ---------------------------------------------------------------- Wild Meridian: savanna under map lines.
scenes['wild-meridian'] = () => {
  const a = new Art(600, 900, 18);
  a.bg(a.lin([[0, '#5a2a68'], [0.32, '#e8604c'], [0.54, '#ffc15e'], [0.66, '#ffe4a0'], [1, '#ffe4a0']]));
  const cx = 300; const cy = 500; const R = 170;
  a.glow(cx, cy, 340, '#fff0c0', 0.8);
  a.circle(cx, cy, R, '#fff3c9', 0.95);
  for (const q of [-0.66, -0.33, 0, 0.33, 0.66]) a.add(`<ellipse cx="${cx}" cy="${cy}" rx="${Math.abs(q) * R + 1}" ry="${R}" fill="none" stroke="#b5502e" stroke-width="2" opacity=".5"/>`);
  for (const q of [-0.5, 0, 0.5]) { const w = Math.sqrt(1 - q * q) * R; a.rect(cx - w, cy + q * R, w * 2, 2, '#b5502e', 0.5); }
  a.path(a.hill(590, [[10, 1.1, 2], [4, 3, 1]]), '#c9742e');
  a.path(a.hill(650, [[14, 0.8, 4], [5, 3, 0.4]]), '#8c4a22');
  a.path(a.hill(752, [[16, 0.9, 0.8], [6, 2.4, 2]]), '#3a1e14');
  const k = '#1c0d0a';
  // An acacia, and a line of animals.
  a.path('M430,660 C434,610 428,580 420,556 L432,556 C440,576 446,596 448,612 C456,588 470,568 486,556 L494,562 C474,590 458,620 452,660 Z', k);
  a.add(`<ellipse cx="456" cy="540" rx="100" ry="24" fill="${k}"/><ellipse cx="392" cy="552" rx="46" ry="13" fill="${k}"/><ellipse cx="520" cy="552" rx="46" ry="13" fill="${k}"/>`);
  [[110, 662, 1], [176, 668, 0.86], [232, 672, 0.74], [278, 676, 0.62]].forEach(([x, y, s]) => {
    a.path(`M${x - 30 * s},${y} L${x - 26 * s},${y - 34 * s} L${x + 20 * s},${y - 36 * s} L${x + 34 * s},${y - 76 * s} L${x + 44 * s},${y - 76 * s} L${x + 42 * s},${y - 30 * s} L${x + 30 * s},${y} L${x + 22 * s},${y} L${x + 20 * s},${y - 22 * s} L${x - 16 * s},${y - 22 * s} L${x - 22 * s},${y} Z`, k);
  });
  a.text(['Wild', 'Meridian'], { x: 44, y: 124, size: 108, lead: 0.92, heavy: 1.2, track: -3, fill: '#fff6e4' });
  return a.finish({ vignette: 0.22 });
};

// ---------------------------------------------------------------- Xeno Farm: a silo and strange crops.
scenes['xeno-farm'] = () => {
  const a = new Art(600, 900, 64);
  a.bg(a.lin([[0, '#0a1140'], [0.36, '#3a1a78'], [0.56, '#c85aa0'], [0.68, '#ffb59a'], [1, '#ffb59a']]));
  a.stars(110, 330, '#fff', 1.4);
  a.circle(450, 300, 84, a.lin([[0.24, '#ffe9c8'], [0.42, '#e8a36a']], 0, 0, 0.3, 0.1));
  a.circle(170, 370, 36, '#b8f2e0');
  a.path(a.hill(600, [[14, 1, 1.2], [6, 3.4, 2]]), '#5c2a7a');
  a.path(a.hill(650, [[16, 0.8, 3.2], [5, 2.8, 0.4]]), '#3a1a5c');
  // Barn and silo.
  const x = 400; const y = 636; const k = '#160a2c';
  a.rect(x - 60, y - 76, 120, 76, k); a.path(poly([[x - 72, y - 76], [x, y - 124], [x + 72, y - 76]]), k);
  a.rect(x + 66, y - 170, 44, 170, k); a.add(`<path d="M${x + 66},${y - 170} A22,22 0 0 1 ${x + 110},${y - 170} Z" fill="${k}"/>`);
  a.rect(x - 16, y - 40, 32, 40, '#7df2c8');
  a.glow(x, y - 20, 90, '#7df2c8', 0.65);
  // The crops, rows of glowing pods.
  for (let row = 0; row < 4; row++) {
    const yy = 680 + row * row * 12 + row * 22; const s = 0.6 + row * 0.34;
    a.path(a.hill(yy + 26 * s, [[3, 2, row], [2, 5, row * 2]]), ['#2a1250', '#201040', '#180a30', '#100620'][row]);
    for (let i = 0; i < 9 - row; i++) {
      const px = 8 + (i + (row % 2) * 0.5) * (600 / (8.4 - row)); const h = 40 * s + a.r() * 18 * s;
      a.line(`M${px},${yy + 26 * s} Q${px + 10 * s},${yy} ${px - 4 * s},${yy - h * 0.5}`, '#37e0a8', 3.4 * s);
      a.glow(px - 4 * s, yy - h * 0.6, 18 * s, '#7df2c8', 0.8);
      a.add(`<ellipse cx="${px - 4 * s}" cy="${yy - h * 0.6}" rx="${8 * s}" ry="${13 * s}" fill="#d0ffe8"/>`);
    }
  }
  a.text(['XENO', 'FARM'], { x: 44, y: 124, size: 112, lead: 0.92, heavy: 2.4, track: 3, fill: '#eafff6' });
  return a.finish({ vignette: 0.24 });
};

// ---------------------------------------------------------------- Yard Sale Hero: a caped figure, a comic sun.
scenes['yard-sale-hero'] = () => {
  const a = new Art(600, 900, 29);
  a.bg('#ffc531');
  const cx = 300; const cy = 480;
  for (let i = 0; i < 24; i += 2) {
    const t = (i / 24) * Math.PI * 2; const t2 = ((i + 1) / 24) * Math.PI * 2;
    a.path(poly([[cx, cy], [cx + Math.cos(t) * 900, cy + Math.sin(t) * 900], [cx + Math.cos(t2) * 900, cy + Math.sin(t2) * 900]]), '#ff8f2e');
  }
  a.path(a.hill(680, [[10, 1.2, 1], [4, 3, 2]]), '#5eb54a');
  // The hero, hands on hips, cape out.
  // A homemade outfit: a tracksuit, a bedsheet for a cape and a price tag for a badge.
  const k = '#0e4a5c';
  a.path(poly([[272, 380], [150, 560], [176, 590], [282, 520]]), '#7a3fd0');
  a.path('M258,380 C262,362 338,362 342,380 L332,490 L268,490 Z', k);
  a.path(poly([[268, 490], [332, 490], [340, 640], [308, 640], [300, 530], [292, 640], [260, 640]]), '#13667a');
  a.path(poly([[258, 382], [226, 450], [270, 474], [274, 450], [256, 442], [276, 400]]), k);
  a.path(poly([[342, 382], [374, 450], [330, 474], [326, 450], [344, 442], [324, 400]]), k);
  a.path(poly([[282, 420], [306, 406], [322, 434], [298, 448]]), '#fff1c4');
  a.circle(290, 423, 4, k);
  a.circle(300, 340, 30, '#ffd9b0');
  a.path('M270,336 C270,300 330,300 330,336 L322,332 L278,332 Z', '#7a3fd0');
  a.rect(250, 636, 50, 14, '#7a3fd0', 1, 6); a.rect(304, 636, 50, 14, '#7a3fd0', 1, 6);
  // The sale table.
  a.rect(40, 690, 520, 18, '#8a5a2a', 1, 4); a.rect(64, 708, 16, 92, '#5a3a1c'); a.rect(520, 708, 16, 92, '#5a3a1c');
  a.rect(70, 630, 90, 60, '#f4e6c8'); a.path(poly([[70, 630], [160, 630], [140, 612], [90, 612]]), '#d8c49c');
  a.circle(232, 668, 22, '#3fb0d0'); a.rect(402, 642, 44, 48, '#fff', 1, 4); a.rect(452, 660, 70, 30, '#e4392e', 1, 4);
  a.text(['YARD SALE', 'HERO'], { x: 300, y: 112, size: 88, lead: 1.02, anchor: 'middle', skew: -6, heavy: 2.4, fill: '#1a2a6c' });
  return a.finish({ vignette: 0, grain: 0.06 });
};

// ---------------------------------------------------------------- Zero Gravity Golf: a flag on the moon.
scenes['zero-gravity-golf'] = () => {
  const a = new Art(600, 900, 42);
  a.bg(a.lin([[0, '#02030c'], [1, '#0e1236']]));
  a.stars(320, 800, '#fff', 1.7);
  a.circle(452, 390, 92, a.lin([[0.34, '#58c6e8'], [0.52, '#1a5a9a']], 0, 0, 0.7, 0.6));
  a.path('M404,340 C430,360 450,340 470,350 C490,380 460,400 440,396 C420,392 396,372 404,340 Z', '#63c77a');
  a.glow(452, 390, 160, '#58c6e8', 0.34);
  a.path(a.hill(660, [[20, 0.7, 2.2], [6, 2.6, 1]]), a.lin([[0.7, '#a7acbc'], [0.95, '#3d4256']]));
  for (const [x, y, rx] of [[120, 690, 60], [420, 712, 88], [260, 770, 40], [530, 780, 50]]) a.add(`<ellipse cx="${x}" cy="${y}" rx="${rx}" ry="${rx * 0.22}" fill="#3d4256" opacity=".75"/>`);
  // The hole, the flag and the long way the ball went.
  a.add(`<ellipse cx="430" cy="660" rx="17" ry="5" fill="#02030c"/>`);
  a.rect(429, 556, 3, 104, '#eef0f6');
  a.path(poly([[432, 556], [484, 572], [432, 588]]), '#ff4a5e');
  a.line('M130,716 C190,330 340,420 416,650', '#ffffff', 3, 0.8, 'stroke-dasharray="1 14"');
  a.circle(258, 440, 13, '#ffffff');
  a.circle(254, 436, 3, '#c9cede');
  // The golfer's suit.
  const k = '#eef0f6';
  a.path('M96,716 L108,640 C112,620 146,620 150,640 L158,716 L134,716 L128,680 L120,716 Z', k);
  a.circle(128, 608, 25, k); a.path('M114,604 C118,592 140,592 144,604 C140,614 118,614 114,604 Z', '#f2a93b');
  a.line('M146,640 L196,670 L176,726', '#3d4256', 4);
  a.text(['ZERO', 'GRAVITY', 'GOLF'], { x: 44, y: 96, size: 80, lead: 0.96, heavy: 1.5, track: 2, fill: '#ffffff' });
  return a.finish({ vignette: 0.2, grain: 0.1 });
};

// Pixels and the part of the 600x900 art to show, for covers that aren't 450x600 from the top of the art.
// Fixtures display up to 382x509 (Large desktop tile at 150 %), and both clients crop to the middle.
const SHAPES = {
  'racing-game': { size: [450, 675], view: [0, 0, 600, 900] },
  'open-world': { size: [480, 480], view: [0, 0, 600, 600] },
  'space-sim': { size: [450, 675], view: [0, 0, 600, 900] },
  'desktop': { top: 100 }, 'music-studio': { top: 100 }, 'retro-arcade': { top: 40 },
  'ember-keep': { top: 30 }, 'ashfall': { top: 30 }, 'deep-field': { top: 30 },
};
const QUALITY = 82;
const slug = (title) => title.toLowerCase().replace(/ /g, '-');

// The demo library's titles in order: their place is the file name, and a game can't lose its cover unnoticed.
function library() {
  const source = readFileSync(resolve(client, 'app/src/debug/java/com/limelight/demo/DemoConsole.java'), 'utf8');
  const block = source.match(/LIBRARY = \{([\s\S]*?)\n\s*\};/);
  if (!block) throw new Error('DemoConsole.LIBRARY not found');
  const slugs = [...block[1].matchAll(/\{"([^"]+)",/g)].map((m) => slug(m[1]));
  const missing = slugs.filter((s) => !scenes[s]);
  const extra = Object.keys(scenes).filter((s) => !slugs.includes(s));
  if (missing.length || extra.length) throw new Error(`Covers and DemoConsole.LIBRARY differ: no scene for [${missing}], no game for [${extra}]`);
  return slugs;
}

async function main() {
  const slugs = library();
  const only = process.argv.slice(2);
  const unknown = only.filter((s) => !slugs.includes(s));
  if (unknown.length) throw new Error(`No such cover: ${unknown}`);
  const art = resolve(client, 'app/src/debug/assets/demo/art'); const work = resolve(client, 'out/demo/assets');
  mkdirSync(work, { recursive: true });
  if (!only.length) rmSync(art, { recursive: true, force: true });
  mkdirSync(art, { recursive: true });
  const font = (weight) => `@font-face{font-family:Figtree;font-weight:${weight};src:url(${pathToFileURL(resolve(client, `protocol/desktop/assets/Figtree-${weight}.ttf`)).href})}`;
  const css = `<style>${font(400)}${font(600)}html,body{margin:0;background:#000}svg{display:block}</style>`;
  const browser = await chromium.launch({ channel: 'chrome', args: ['--disable-gpu', '--disable-lcd-text', '--force-color-profile=srgb'] });
  const tmp = resolve(work, '_cover.html');
  for (const [i, name] of slugs.entries()) {
    if (only.length && !only.includes(name)) continue;
    const { size = [450, 600], top = 0, view = [0, top, 600, 800] } = SHAPES[name] ?? {};
    const page = await browser.newPage({ viewport: { width: size[0], height: size[1] }, deviceScaleFactor: 1 });
    writeFileSync(tmp, css + scenes[name]().replace(/width="\d+" height="\d+" viewBox="[^"]+"/, `width="${size[0]}" height="${size[1]}" viewBox="${view.join(' ')}"`));
    await page.goto(pathToFileURL(tmp).href);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: resolve(art, `${i + 1}.jpg`), type: 'jpeg', quality: QUALITY });
    await page.close();
  }
  // Every cover as a desktop tile shows it at 100 %.
  const files = readdirSync(art).sort((x, y) => parseInt(x, 10) - parseInt(y, 10));
  const sheet = await browser.newPage({ viewport: { width: 1846, height: 1130 }, deviceScaleFactor: 1 });
  writeFileSync(tmp, `<body style="margin:0;background:#0b0d12;display:grid;grid-template-columns:repeat(9,196px);grid-auto-rows:261px;gap:8px;padding:8px">${files.map((f) => `<img src="${pathToFileURL(resolve(art, f)).href}" style="width:196px;height:261px;object-fit:cover;border-radius:10px">`).join('')}</body>`);
  await sheet.goto(pathToFileURL(tmp).href);
  await sheet.waitForLoadState('networkidle');
  await sheet.screenshot({ path: resolve(work, 'covers-contact.png') });
  rmSync(tmp);
  await browser.close();
}
main().catch((e) => { console.error(e); process.exit(1); });
