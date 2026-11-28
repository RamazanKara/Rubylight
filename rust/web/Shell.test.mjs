import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's shell: the rail, the command bar above the page, and its live badge.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { liveBadge } = await server.ssrLoadModule('/src/lib/shell.ts');
const sessionMod = await server.ssrLoadModule('/src/lib/session.svelte.ts');

const source = name => readFileSync(new URL(`./src/${name}`, import.meta.url), 'utf8');

test('the live badge says what plays now, and nothing while the PC idles', () => {
  assert.equal(liveBadge(null), null, 'the host has not said yet');
  assert.equal(liveBadge({ streams: 0, app: '' }), null);
  // A game that runs with no device watching is not live.
  const running = liveBadge({ streams: 0, app: 'Portal 2' });
  assert.deepEqual([running.label, running.streaming], ['Portal 2 is running', false]);
  const live = liveBadge({ streams: 1, app: 'Portal 2' });
  assert.deepEqual([live.label, live.streaming], ['Live · Portal 2', true]);
  assert.match(live.title, /^One device is streaming\./);
  // Streams without a named app (the desktop) count the streams instead.
  assert.equal(liveBadge({ streams: 1, app: '' }).label, 'Live · 1 stream');
  const two = liveBadge({ streams: 2, app: '' });
  assert.equal(two.label, 'Live · 2 streams');
  assert.match(two.title, /^2 streams are running\./);
});

test('the badge is fed by the shell’s poll, marked so an open console lets the PC sleep', () => {
  const session = source('lib/session.svelte.ts');
  assert.match(session, /await marked\(\(\) => api\.sessions\.status\(\)\)/);
  // A failed status never becomes an error of its own: the badge goes, the pages report.
  assert.match(session, /\} catch \{\s*session\.live = null;/);
  assert.match(session, /session\.pending = \[\];\s*session\.live = null;/, 'signing out clears it');
});

test('Search, the live badge and the theme sit in the command bar, and in the menu on a phone', () => {
  const app = source('App.svelte');
  // One Search and one theme switch at a time: no hidden duplicate for a screen reader or a test to find.
  assert.match(app, /\{#if compact\}\{@render search\(\)\}\{\/if\}/);
  assert.match(app, /\{#if compact\}<ThemeSwitch \{theme\} onchange=\{setTheme\} \/>\{\/if\}/);
  assert.match(app, /\{#if !compact\}[\s\S]*?<div class="bar" role="toolbar" aria-label="Console">[\s\S]*?\{@render search\(\)\}[\s\S]*?<LiveBadge \{badge\} \/>[\s\S]*?<ThemeSwitch/);
  assert.match(app, /matchMedia\('\(max-width: 860px\)'\)/);
  assert.match(app, /@media \(max-width: 860px\)/, 'the same width as the stylesheet');
  // The bar scrolls with the page, so a page's own sticky parts keep their offsets.
  const bar = app.match(/\n  \.bar \{[^}]*\}/)[0];
  assert.doesNotMatch(bar, /position:/);
  const theme = source('components/shell/ThemeSwitch.svelte');
  assert.match(theme, /role="group" aria-label="Theme"/);
  assert.match(theme, /aria-pressed=\{theme === option\.value\}/);
});

test('Overview and the shell heartbeat share one /api/session/status poll', async () => {
  const overview = source('pages/Overview.svelte');
  assert.match(overview, /fetchSessionStatus\(\)/);
  assert.doesNotMatch(overview, /api\.sessions\.status\(\)/);
  const mod = sessionMod;
  const realFetch = globalThis.fetch;
  let statusCalls = 0;
  globalThis.fetch = async (url) => {
    const path = String(url);
    if (path.endsWith('/api/session/status')) {
      statusCalls++;
      return new Response(
        JSON.stringify({ status: true, activeSessions: 1, appRunning: true, appName: 'Portal 2', appId: 1 }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    }
    return new Response(JSON.stringify({ status: true, requests: [] }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  };
  try {
    mod.session.state = 'signed-in';
    const [a, b] = await Promise.all([mod.fetchSessionStatus(), mod.fetchSessionStatus()]);
    assert.equal(a.appName, 'Portal 2');
    assert.equal(b.appName, 'Portal 2');
    assert.equal(statusCalls, 1, 'concurrent callers coalesce onto one request');
    // Immediately afterwards, heartbeat skips a duplicate /api/session/status poll.
    await mod.heartbeat();
    assert.equal(statusCalls, 1, 'heartbeat reuses fresh Overview status');
    assert.deepEqual(mod.session.live, { streams: 1, app: 'Portal 2' });
  } finally {
    globalThis.fetch = realFetch;
  }
});
