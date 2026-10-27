import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const css = readFileSync(new URL('./src/styles/tokens.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

function block(selector, from = 0) {
  const start = css.indexOf(selector + ' {', from);
  assert.notEqual(start, -1, `tokens.css has no ${selector} block`);
  const body = css.slice(start + selector.length + 2, css.indexOf('}', start));
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()]));
}

const light = block(':root');
const dark = block(":root[data-theme='dark']");
const systemDark = block(":root[data-theme='system']", css.indexOf('prefers-color-scheme: dark'));
const themes = { light, dark };

function luminance(hex) {
  assert.match(hex, /^#[0-9a-f]{6}$/i, `${hex} is not a #rrggbb colour`);
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(theme, fg, bg) {
  const value = (name) => theme[name] ?? light[name];
  const [a, b] = [luminance(value(fg)), luminance(value(bg))];
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

const surfaces = ['--bg', '--panel', '--panel-raised', '--sunken'];
const tints = ['--accent-soft', '--ok-soft', '--warn-soft', '--danger-soft'];
// Every pair the console draws text with. Status colours sit on surfaces and on their own tint.
const textPairs = [
  ...['--ink', '--ink-2', '--muted'].flatMap((fg) => [...surfaces, ...tints].map((bg) => [fg, bg])),
  ...['--ok', '--warn', '--danger', '--accent-text', '--live'].flatMap((fg) => surfaces.map((bg) => [fg, bg])),
  ['--ok', '--ok-soft'], ['--warn', '--warn-soft'], ['--danger', '--danger-soft'], ['--accent-text', '--accent-soft'],
  ['--accent-ink', '--accent'], ['--accent-ink', '--accent-strong'],
  ['--media-ink', '--media-bg'], ['--media-ink-2', '--media-bg'], ['--media-idle', '--media-bg'],
];
// Non-text UI: focus rings, the off switch outline and the on switch track need 3:1 (WCAG 1.4.11).
const uiPairs = [
  ...surfaces.map((bg) => ['--focus', bg]),
  ...surfaces.map((bg) => ['--muted', bg]),
  ['--accent', '--panel'], ['--accent', '--bg'],
  ['--media-ok', '--media-bg'], ['--media-warn', '--media-bg'],
];

for (const [name, theme] of Object.entries(themes)) {
  test(`${name} theme text meets 4.5:1 on every surface it is drawn on`, () => {
    const failures = textPairs.map(([fg, bg]) => [fg, bg, contrast(theme, fg, bg)]).filter(([, , ratio]) => ratio < 4.5);
    assert.deepEqual(failures.map(([fg, bg, ratio]) => `${fg} on ${bg}: ${ratio.toFixed(2)}`), []);
  });
  test(`${name} theme focus rings and control outlines meet 3:1`, () => {
    const failures = uiPairs.map(([fg, bg]) => [fg, bg, contrast(theme, fg, bg)]).filter(([, , ratio]) => ratio < 3);
    assert.deepEqual(failures.map(([fg, bg, ratio]) => `${fg} on ${bg}: ${ratio.toFixed(2)}`), []);
  });
}

test('the system dark theme is the same as the chosen dark theme', () => {
  assert.deepEqual(systemDark, dark);
});

test('the dark theme sets every colour the light theme sets', () => {
  // Media colours sit on game art and video, so they are the same in both themes.
  const colours = Object.keys(light).filter((name) => !name.startsWith('--media-')
    && (/^#|^rgb/.test(light[name]) || name.startsWith('--shadow')));
  assert.deepEqual(colours.filter((name) => !(name in dark)), []);
});

test('reduced motion zeroes every duration token', () => {
  const reduced = block(':root', css.indexOf('prefers-reduced-motion'));
  const durations = Object.keys(light).filter((name) => name.startsWith('--duration-'));
  assert.ok(durations.length >= 3);
  for (const name of durations) assert.equal(reduced[name], '0ms', name);
});

test('scales only grow', () => {
  const px = (names) => names.map((name) => parseFloat(light[name]));
  const ascending = (values) => values.every((value, i) => i === 0 || value > values[i - 1]);
  assert.ok(ascending(px(['--text-xs', '--text-sm', '--text', '--text-md', '--text-lg', '--text-xl', '--text-2xl'])));
  assert.ok(ascending(px(Array.from({ length: 8 }, (_, i) => `--space-${i + 1}`))));
  assert.ok(ascending(px(['--radius-sm', '--radius', '--radius-lg'])));
  assert.ok(ascending(px(['--duration-fast', '--duration-base', '--duration-slow'])));
});
