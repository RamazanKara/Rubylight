import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { default: LogFindings } = await server.ssrLoadModule('/src/components/maintenance/LogFindings.svelte');
const { default: ClientAgent } = await server.ssrLoadModule('/src/components/devices/ClientAgent.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

const finding = {
  id: 'encoder_hang',
  category: 'encoder',
  level: 'error',
  title: 'The AMD encoder stopped answering',
  explanation: 'An AMF call took seconds.',
  fix: 'Update the AMD driver.',
  link: 'https://github.com/RamazanKara/Rubylight/blob/main/docs/troubleshooting.md#a-driver-is-missing-or-out-of-date',
  count: 3,
  first_seen: '2026-10-06T17:52:03.632096Z',
  last_seen: '2026-10-07T09:00:00.000000Z',
  sample: 'butterpollo::stream: slow encoder call call="AMF SubmitInput" elapsed_ms=34787',
};
const findings = (list, since = '2026-10-06T00:00:00Z') =>
  render(LogFindings, { props: { analysis: { status: true, findings: list, since, until: since, read_bytes: 10 } } }).body;

test('log analysis lists each finding with its count, fix and guide link', () => {
  const html = findings([finding, { ...finding, id: 'roaming', category: 'network', level: 'warn', count: 1, title: 'The client changed networks' }]);
  assert.match(html, /The AMD encoder stopped answering/);
  assert.match(html, /3 times, last /);
  assert.match(html, /Once, last /);
  assert.match(html, /Update the AMD driver\./);
  assert.match(html, /Encoder/);
  assert.match(html, /Network/);
  assert.match(html, /href="https:\/\/github\.com\/RamazanKara\/Rubylight\/blob\/main\/docs\/troubleshooting\.md#a-driver-is-missing-or-out-of-date"/);
  assert.match(html, /target="_blank" rel="noreferrer"/);
  assert.match(html, /elapsed_ms=34787/);
});

test('log analysis says so when the log has no known problems', () => {
  assert.match(findings([]), /No known problems in the log since /);
  assert.match(findings([], null), /No known problems in the log\./);
});

test('log analysis escapes log text', () => {
  const html = findings([{ ...finding, sample: 'client=<script>alert(1)</script>' }]);
  assert.match(html, /&lt;script/);
  assert.doesNotMatch(html, /<script>alert/);
});

test('the device list names the client and warns only about listed builds', () => {
  const agent = (client) => render(ClientAgent, { props: { client } }).body;
  const plain = agent({ client_agent: 'Nightjar', client_warning: null });
  assert.match(plain, /Client: <span class="mono">Nightjar<\/span>/);
  assert.doesNotMatch(plain, /known problem/);
  assert.doesNotMatch(agent({}), /Client:/);
  const warned = agent({
    client_agent: 'Nightjar/3.1.2 <b>',
    client_warning: { problem: 'freezes after a keyframe request', link: 'https://github.com/RamazanKara/Rubylight/issues/1' },
  });
  assert.match(warned, /known problem: freezes after a keyframe request\. It can still connect\./);
  assert.match(warned, /href="https:\/\/github\.com\/RamazanKara\/Rubylight\/issues\/1"/);
  assert.match(warned, /Nightjar\/3\.1\.2 &lt;b>/);
});
