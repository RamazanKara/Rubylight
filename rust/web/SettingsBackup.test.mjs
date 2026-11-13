import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { backupFileName, changeLabel, fileProblem, restoreNote, BACKUP_LIMIT } = await server.ssrLoadModule(
  '/src/components/maintenance/backup.ts',
);
const inspection = secrets => ({
  status: true,
  token: '0'.repeat(32),
  summary: { created_unix: 1_700_000_000, host_version: '4.1.0', secrets, changes: [] },
});

test('a backup file is named by its date and says when it holds secrets', () => {
  const when = new Date(2026, 10, 13, 9, 5, 7);
  assert.equal(backupFileName(false, when), 'rubylight-settings-20261113-090507.zip');
  assert.equal(backupFileName(true, when), 'rubylight-settings-20261113-090507-with-secrets.zip');
});

test('only a non-empty .zip within the host limit is uploaded', () => {
  assert.equal(fileProblem({ name: 'rubylight-settings.zip', size: 1024 }), '');
  assert.equal(fileProblem({ name: 'BACKUP.ZIP', size: 1 }), '');
  assert.match(fileProblem({ name: 'a.zip', size: 0 }), /empty/);
  assert.match(fileProblem({ name: 'a.zip', size: BACKUP_LIMIT + 1 }), /larger/);
  assert.match(fileProblem({ name: 'sunshine.conf', size: 10 }), /\.zip/);
});

test('the restore note warns about a pairing identity only when the backup has one', () => {
  const plain = restoreNote(inspection(false));
  assert.match(plain, /Rubylight 4\.1\.0/);
  assert.match(plain, /saves the current settings first/);
  assert.doesNotMatch(plain, /pairing identity/);
  assert.match(restoreNote(inspection(true)), /pairing identity and sign-in: this PC takes them over/);
  for (const item of ['settings', 'apps', 'covers', 'game_settings', 'devices', 'pairing', 'credentials', 'webhooks']) {
    assert.notEqual(changeLabel(item), item, item);
  }
});

test('secrets are off until chosen, warned about, and confirmed before the download', async () => {
  const panel = await readFile(new URL('./src/components/maintenance/BackupPanel.svelte', import.meta.url), 'utf8');
  assert.match(panel, /let secrets = \$state\(false\);/);
  assert.match(panel, /label="Include pairing identity and sign-in"/);
  assert.match(panel, /\{#if secrets\}\s*<Banner tone="warn">/);
  assert.match(panel, /title: 'Include secrets\?'/);
  assert.doesNotMatch(panel, /Experimental|beta|preview/i);
});
