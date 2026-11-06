// Proves the numeric promises of ../tokens.json, the single source. Writes nothing; exits 1 on any miss.
//   node check-tokens.mjs [path/to/tokens.json]
// Text reaches 4.5:1 on each theme's worst backdrop (white for night and OLED, black for day), the no-art tiles carry
// a white title, the focus ring stands off its gap, and every spring settles inside the navigation budget.
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const here = dirname(fileURLToPath(import.meta.url));
const t = JSON.parse(readFileSync(process.argv[2] || resolve(here, '../tokens.json'), 'utf8'));

const hex = (s) => [1, 3, 5, 7].map((i) => (s.length > i ? parseInt(s.slice(i, i + 2), 16) : 255));
const lin = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
const lum = (v) => 0.2126 * lin(v[0]) + 0.7152 * lin(v[1]) + 0.0722 * lin(v[2]);
const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
const over = (top, under) => { const a = top[3] / 255; return under.map((c, i) => top[i] * a + c * (1 - a)); };
const lumaText = (text, luma) => { const l = lum(text); const [x, y] = [l, luma].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };

let bad = 0;
const line = (text, values, min = 4.5) => {
  const fail = values.some((v) => v < min);
  if (fail) bad++;
  console.log(`${text.padEnd(40)} ${values.map((v) => v.toFixed(2)).join(' ')} ${fail ? `FAIL (needs ${min})` : ''}`);
};

// label, secondary, then each at the sheen's peak. Chrome and chip keep secondary text below the sheen's end.
function glass(prefix, material, tier, backdrop, label, secondary) {
  const g = (k) => t[`${prefix}glass.${material}.${tier}.${k}`];
  if (g('fill') === undefined) return;
  const base = over(hex(g('fill')), backdrop.map((c) => Math.min(255, c * g('brightness'))));
  const peak = over(hex(g('sheenTop')), base);
  const values = [contrast(label, base), contrast(secondary, base), contrast(label, peak)];
  if (['panel', 'menu'].includes(material)) values.push(contrast(secondary, peak));
  line(`${prefix || 'night.'}${material}.${tier}`, values);
}
const WHITE = [255, 255, 255]; const BLACK = [0, 0, 0];
const night = [hex(t['color.label']), hex(t['color.labelSecondary'])];
const day = [hex(t['day.color.label']), hex(t['day.color.labelSecondary'])];
for (const m of ['chrome', 'panel', 'menu', 'chip']) {
  for (const tier of ['lens', 'live', 'baked']) glass('', m, tier, WHITE, ...night);
  glass('', m, 'solid', WHITE, hex(t['hc.color.label']), hex(t['hc.color.labelSecondary']));
  for (const tier of ['lens', 'live', 'baked']) glass('day.', m, tier, BLACK, ...day);
}
glass('', 'menu', 'window', WHITE, ...night);
line('stats on a white frame, label', [contrast(night[0], over(hex(t['stats.fill']), WHITE))]);

// Ambient: text straight on the bake, at the clamp.
line('night ambient, ceiling', night.map((c) => lumaText(c, t['ambient.textZoneMaxLuma'])));
line('day ambient, floor', day.map((c) => lumaText(c, t['day.ambient.textZoneMinLuma'])));

// Selection and the moon.
line('night selected on chrome over white', [contrast(hex(t['color.onSelected']), over(hex(t['tint.selected.fill']), over(hex(t['glass.chrome.live.fill']), WHITE.map((c) => c * t['glass.chrome.live.brightness']))))]);
line('day selected on chrome over black', [contrast(hex(t['day.color.onSelected']), over(hex(t['day.tint.selected.fill']), over(hex(t['day.glass.chrome.live.fill']), BLACK)))]);
line('moon label', [contrast(hex(t['color.onMoon']), hex(t['play.fillTop'])), contrast(hex(t['color.onMoon']), hex(t['play.fillBottom']))]);
line('day moon ink on canvas', [contrast(hex(t['day.color.moonInk']), hex(t['day.color.canvas']))]);
line('day status ok, error on canvas', [contrast(hex(t['day.color.ok']), hex(t['day.color.canvas'])), contrast(hex(t['day.color.error']), hex(t['day.color.canvas']))]);
line('danger fill', [contrast(hex(t['color.onDangerFill']), hex(t['color.dangerFill'])), contrast(hex(t['day.color.onDangerFill']), hex(t['day.color.dangerFill']))]);

// Focus: a ring has to stand off its gap, 3:1 for non-text.
line('focus ring on gap, night', [contrast(hex(t['focus.ringColor']), over(hex(t['focus.gapColor']), BLACK))], 3);
line('focus ring on gap, day', [contrast(hex(t['day.focus.ringColor']), hex(t['day.focus.gapColor']))], 3);

// No-art tiles: white title on both stops, with the disc over the lighter one.
const disc = [255, 255, 255, Math.round(t['tile.discAlpha'] * 255)];
for (const key of Object.keys(t).filter((k) => /^tile\.pair\d+\.top$/.test(k))) {
  const top = hex(t[key]); const bottom = hex(t[key.replace('top', 'bottom')]);
  line(key.replace('.top', ''), [contrast(WHITE, top), contrast(WHITE, over(disc, top)), contrast(WHITE, bottom)]);
}

// Springs: settle = 4 / (ratio * sqrt(stiffness)).
for (const m of ['press', 'focus', 'tabPill', 'nav', 'sheet', 'switch']) {
  const s = t[`motion.${m}.stiffness`]; const r = t[`motion.${m}.ratio`];
  const settle = (4 / (Math.min(r, 1) * Math.sqrt(s))) * 1000;
  const fail = settle > t['motion.navMaxMs'];
  if (fail) bad++;
  console.log(`spring ${m.padEnd(33)} ${settle.toFixed(0)} ms ${fail ? 'FAIL' : ''}`);
}
if (bad) { console.error(`${bad} checks failed`); process.exit(1); }
console.log('all checks pass');
