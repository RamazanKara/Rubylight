import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

// Rubylight 7.0: raw colours, shadows, radii, layers and durations live in src/styles/tokens.css
// and nowhere else (DESIGN.md, "Tokens"). This reads every stylesheet and every component's
// <style> block and inline style, and names each declaration that carries a raw value.
const root = fileURLToPath(new URL('./src/', import.meta.url));
const TOKENS = join('styles', 'tokens.css');

const files = dir => readdirSync(dir).flatMap(name => {
  const path = join(dir, name);
  return statSync(path).isDirectory() ? files(path) : /\.(svelte|css)$/.test(name) ? [path] : [];
});

/** A file's CSS with comments blanked, so offsets still give line numbers. */
function styles(path, source) {
  const blank = text => text.replace(/[^\n]/g, ' ');
  if (path.endsWith('.css')) return source.replace(/\/\*[\s\S]*?\*\//g, blank);
  // Everything but the <style> blocks is blanked; inline styles are checked apart.
  let css = '';
  let last = 0;
  for (const match of source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) {
    const start = match.index + match[0].indexOf('>') + 1;
    css += blank(source.slice(last, start)) + match[1];
    last = start + match[1].length;
  }
  return (css + blank(source.slice(last))).replace(/\/\*[\s\S]*?\*\//g, blank);
}

const COLOUR = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![-\w])(?:white|black|red|gray|grey|silver)(?![-\w(])/i;
const TIME = /(?<![\w.-])(\d*\.?\d+)(ms|s)\b/g;
const TOKEN_ONLY = /^(?:(?:var\(--[\w-]+\)|none|inherit|initial|unset)(?:\s*,\s*)?)+$/;
/** Values that are no literal: a token, a keyword, or arithmetic on a token. */
const radiusOk = value => value.replace(/calc\(var\(--radius[\w-]*\) [-+] \d+px\)/g, 'var(--radius)').split(/\s+|\//).filter(Boolean)
  .every(part => /^(?:0|50%|100%|inherit|initial|unset|var\(--radius[\w-]*\))$/.test(part));
const layerOk = value => /^(?:auto|0|inherit|initial|unset|var\(--z-[\w-]+\)|calc\(var\(--z-[\w-]+\) [-+] \d+\))$/.test(value);

/** Why a declaration breaks the rule, or nothing. */
function fault(property, raw) {
  // A colour derived from a token (relative colour syntax, color-mix of tokens) is no literal.
  const value = raw.replace(/\b(?:oklch|oklab|rgb|hsl)\(from var\(--[\w-]+\)(?:[^()]|\([^()]*\))*\)/g, 'derived');
  if (property.startsWith('--')) return COLOUR.test(value) ? 'a raw colour in a local custom property' : null;
  if (COLOUR.test(value)) return 'a raw colour';
  if (/^(?:box-shadow|text-shadow)$/.test(property) && !TOKEN_ONLY.test(value)) return 'a raw shadow (use a --shadow, --edge or --ring token)';
  if (/drop-shadow\(/.test(value)) return 'a raw drop shadow';
  if (/^border(?:-[a-z]+)*-radius$/.test(property) && !radiusOk(value)) return 'a raw radius';
  if (property === 'z-index' && !layerOk(value)) return 'a raw layer';
  // Type and space come from the scale (--text-*, --space-*); a fluid clamp() or an em stays free.
  if (property === 'font-size' && /(?<![\w.(,-])\d*\.?\d+px\b/.test(value.replace(/(?:calc|clamp|min|max)\([^;]*\)/g, ''))) {
    return 'a raw type size (use a --text token)';
  }
  if (/^(?:(?:padding|margin)(?:-[a-z]+)*|gap|row-gap|column-gap)$/.test(property)) {
    // Arithmetic on a token is no literal; a nudge of 1 to 3px aligns a hairline and is let be.
    let bare = value;
    for (let start; (start = bare.indexOf('calc(')) >= 0;) {
      let end = start + 5;
      for (let depth = 1; depth && end < bare.length; end++) depth += { '(': 1, ')': -1 }[bare[end]] ?? 0;
      const group = bare.slice(start, end);
      bare = bare.slice(0, start) + (group.includes('var(--') ? '' : group.replace('calc(', 'calc[')) + bare.slice(end);
    }
    if ([...bare.matchAll(/(?<![\w.-])-?(\d*\.?\d+)px\b/g)].some(([, amount]) => Number(amount) > 3)) {
      return 'a raw space (use a --space token)';
    }
  }
  if (/^(?:transition|animation)(?:-duration|-delay)?$/.test(property)) {
    for (const [, amount] of value.replace(/var\([^)]*\)/g, '').matchAll(TIME)) {
      if (Number(amount) !== 0) return 'a raw duration';
    }
  }
  return null;
}

function faults(path, source) {
  const found = [];
  const line = (text, index) => text.slice(0, index).split('\n').length;
  const css = styles(path, source);
  for (const match of css.matchAll(/(?<![\w-])(--[\w-]+|[a-z-]+)\s*:\s*([^;{}]+?)\s*(?:!important\s*)?(?=[;}])/g)) {
    const [, property, value] = match;
    // A selector's pseudo-class (a:hover) is no declaration; a real one starts a line or follows ; or {.
    if (!/(?:^|[;{}\n])\s*$/.test(css.slice(Math.max(0, match.index - 200), match.index))) continue;
    const why = fault(property, value.replace(/\s+/g, ' '));
    if (why) found.push(`${path}:${line(css, match.index)} ${property}: ${value.replace(/\s+/g, ' ')} is ${why}`);
  }
  if (path.endsWith('.svelte')) {
    const markup = source.replace(/<style[^>]*>[\s\S]*?<\/style>/g, text => text.replace(/[^\n]/g, ' '))
      .replace(/<script[^>]*>[\s\S]*?<\/script>/g, text => text.replace(/[^\n]/g, ' '));
    for (const match of markup.matchAll(/\b(?:style|fill|stroke|stop-color|flood-color|color)=(?:"([^"]*)"|'([^']*)')/g)) {
      const value = (match[1] ?? match[2]).replace(/\{[^}]*\}/g, '');
      if (COLOUR.test(value)) found.push(`${path}:${line(markup, match.index)} ${match[0]} carries a raw colour`);
    }
  }
  return found;
}

const sources = files(root).map(path => [relative(root, path), readFileSync(path, 'utf8')]);

test('the walk finds the console, /play and /guest', () => {
  const names = sources.map(([path]) => path.replaceAll('\\', '/'));
  for (const expected of ['App.svelte', 'play/Play.svelte', 'guest/Guest.svelte', 'styles/base.css', 'styles/tokens.css']) {
    assert.ok(names.includes(expected), expected);
  }
  assert.ok(names.length > 100);
});

test('no stylesheet or component outside tokens.css carries a raw colour, shadow, radius, layer, duration, type size or space', () => {
  const found = sources.filter(([path]) => path !== TOKENS).flatMap(([path, source]) => faults(path.replaceAll('\\', '/'), source));
  assert.deepEqual(found, []);
});

test('the rule catches what it is for', () => {
  const bad = `<div style="color: #fff"></div><svg fill="rgb(1 2 3)"></svg><style>
    .a { color: #c41242; }
    .b { background: rgb(0 0 0 / 40%); }
    .c { box-shadow: 0 1px 2px var(--line); }
    .d { border-radius: 6px; }
    .e { z-index: 100; }
    .f { transition: color 120ms; }
    .g { animation: spin 0.7s linear infinite; }
    .h { --local: #123456; }
    .i { color: white; }
    .j { filter: drop-shadow(0 1px 2px var(--scrim)); }
    .k { font-size: 12px; }
    .l { padding: 10px var(--space-4); }
    .m { gap: 6px; margin-block: -8px; }
  </style>`;
  assert.equal(faults('Bad.svelte', bad).length, 16);
  const good = `<div style="width: {size}px; color: var(--ink)"></div><svg fill="currentColor"></svg><style>
    a:hover { color: var(--accent-text); }
    .a { background: color-mix(in srgb, var(--accent) 20%, transparent); }
    .b { box-shadow: var(--edge), var(--shadow); }
    .c { border-radius: var(--radius-lg) var(--radius-lg) 0 0; }
    .d { border-radius: 50%; }
    .e { border-radius: calc(var(--radius-lg) - 1px); }
    .f { z-index: var(--z-sticky); }
    .g { z-index: calc(var(--z-sticky) + 1); }
    .h { transition: color var(--duration-fast) var(--ease-standard), transform 0s; }
    .i { animation: spin var(--spin-duration) linear infinite; }
    .j { white-space: nowrap; grid-template-rows: 0fr; }
    .k { font-size: var(--text-sm); padding: calc(var(--space-3) - 1px) var(--space-4); margin-top: 2px; }
    .l { font-size: clamp(16px, 36cqi, 60px); gap: 0.5em; margin-block: calc((20px - var(--control-height)) / 2); }
    /* a comment may say #fff or 120ms */
  </style>`;
  assert.deepEqual(faults('Good.svelte', good), []);
});
