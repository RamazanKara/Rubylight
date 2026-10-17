import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { applyTheme, storedTheme } = await server.ssrLoadModule('/src/lib/theme.ts');

test('a browser that blocks site data still gets a console, in the system theme', () => {
  globalThis.document = { documentElement: { dataset: {} } };
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    get() { throw new Error('SecurityError: Access is denied for this document.'); },
  });
  try {
    assert.equal(storedTheme(), 'system');
    assert.doesNotThrow(() => applyTheme('dark'));
    assert.equal(document.documentElement.dataset.theme, 'dark');
  } finally {
    delete globalThis.localStorage;
    delete globalThis.document;
  }
});

test('the chosen theme is remembered, and System clears the memory', () => {
  const stored = new Map();
  globalThis.document = { documentElement: { dataset: {} } };
  globalThis.localStorage = {
    getItem: key => stored.get(key) ?? null,
    setItem: (key, value) => stored.set(key, value),
    removeItem: key => stored.delete(key),
  };
  try {
    applyTheme('light');
    assert.equal(storedTheme(), 'light');
    applyTheme('system');
    assert.equal(storedTheme(), 'system');
    assert.equal(stored.size, 0);
  } finally {
    delete globalThis.localStorage;
    delete globalThis.document;
  }
});
