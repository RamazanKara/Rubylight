// Writes the prototype's tokens.css (gitignored) from ../tokens.json. The apps use scripts/gen_design_tokens.py instead.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const here = dirname(fileURLToPath(import.meta.url));
const t = JSON.parse(readFileSync(resolve(here, '../tokens.json'), 'utf8'));
const hex = (s) => [1, 3, 5, 7].map((i) => (s.length > i ? parseInt(s.slice(i, i + 2), 16) : 255));
const rgba = (s) => { const h = hex(s); return `rgba(${h[0]},${h[1]},${h[2]},${(h[3] / 255).toFixed(3)})`; };
const isPx = (k, v) => typeof v === 'number' && (/^(radius|space|size|layout)\./.test(k) || /^focus\.(ring|gap|halo)/.test(k) || /shadow[YB]|glowBlur/.test(k));
const value = (k, v) => `${typeof v === 'string' && v.startsWith('#') ? rgba(v) : v}${isPx(k, v) ? 'px' : ''}`;
const prop = (k) => '--' + k.replace(/\./g, '-');
const block = (selector, prefix) => {
  let out = `${selector}{\n`;
  for (const [k, v] of Object.entries(t)) {
    if (!k.startsWith(prefix) || k.startsWith(prefix + 'glass.') || k.startsWith('$')) continue;
    if (!prefix && /^(hc|day|oled)\./.test(k)) continue;
    const bare = k.slice(prefix.length);
    out += `  ${prop(bare)}:${value(bare, v)};\n`;
  }
  return out + '}\n';
};
let css = block(':root', '') + block('[data-theme="day"]', 'day.') + block('[data-theme="oled"]', 'oled.') + block('[data-hc="1"]', 'hc.');
const recipe = (selector, g) => `${selector}{--fill:${rgba(g('fill'))};--blur:${g('blur')}px;--sat:${g('saturate')};--bri:${g('brightness')};--sheen:${rgba(g('sheenTop'))};--sheen-end:${g('sheenEnd') * 100}%;--rim-t:${rgba(g('rimTop'))};--rim-m:${rgba(g('rimMid'))};--rim-b:${rgba(g('rimBottom'))};--rim-w:${g('rimWidth')}px;--sh:${g('hairline') ? `0 0 0 1px ${rgba(g('hairline'))},` : ''}0 ${g('shadowY')}px ${g('shadowBlur')}px ${rgba(g('shadowColor'))};--lens-w:${g('lensWidth')}px;--lens-blur:${g('lensBlur')}px;--lens-sat:${g('lensSaturate')};--lens-bri:${g('lensBrightness')};}\n`;
for (const name of ['chrome', 'panel', 'menu', 'chip']) {
  for (const tier of ['lens', 'live', 'baked', 'solid']) {
    css += recipe(`[data-tier="${tier}"] .g.${name}`, (k) => t[`glass.${name}.${tier}.${k}`]);
    if (t[`day.glass.${name}.${tier}.fill`]) css += recipe(`[data-theme="day"][data-tier="${tier}"] .g.${name}`, (k) => t[`day.glass.${name}.${tier}.${k}`]);
  }
}
writeFileSync(resolve(here, 'tokens.css'), css);
