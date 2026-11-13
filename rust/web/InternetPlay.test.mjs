import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/InternetPlayPanel.svelte')).default;
const { settings } = await server.ssrLoadModule('/src/lib/schema/basics.ts');
const { api } = await server.ssrLoadModule('/src/lib/api.ts');

const html = (initial) => render(Panel, { props: { initial } }).body;

const base = {
  enabled: true,
  portMapping: true,
  mapping: {
    enabled: true,
    method: 'PCP',
    externalIpv4: '203.0.113.5',
    mapped: [{ protocol: 'TCP', port: 47984, external: 47984, reason: null }],
    failed: [],
    error: null,
    ipv6Method: null,
    ipv6Pinholes: 0,
    ipv6Error: null,
  },
  externalIp: '203.0.113.5',
  rendezvous: {
    public: ['203.0.113.5:40000'],
    nat: 'cone',
    stunError: null,
    devices: 2,
    published: 2,
    dhtError: null,
    paths: [{ device: 'Phone', peer: '198.51.100.7:50000', connected: true }],
  },
  notes: [],
};

test('the internet status shows the mapping method, the public address and connected devices', () => {
  const out = html(base);
  assert.match(out, /Mapped with PCP/);
  assert.match(out, /TCP 47984/);
  assert.match(out, /203\.0\.113\.5/);
  assert.match(out, /Reachable/);
  assert.match(out, /2 of 2 can find this PC/);
  assert.match(out, /Phone/);
  assert.match(out, /Connected directly/);
});

test('a failed mapping says why, per port and for the router', () => {
  const partly = {
    ...base,
    mapping: {
      ...base.mapping,
      failed: [{ protocol: 'UDP', port: 47998, external: null, reason: 'the router is out of mappings (PCP 8)' }],
    },
  };
  assert.match(html(partly), /Partly mapped with PCP/);
  assert.match(html(partly), /UDP 47998/);
  assert.match(html(partly), /out of mappings/);

  const none = {
    ...base,
    mapping: { ...base.mapping, method: null, mapped: [], error: 'the router answers none of UPnP, PCP and NAT-PMP' },
  };
  assert.match(html(none), /Failed/);
  assert.match(html(none), /answers none of UPnP, PCP and NAT-PMP/);
});

test('carrier-grade NAT and an unreachable rendezvous are explained', () => {
  const out = html({
    ...base,
    rendezvous: { ...base.rendezvous, stunError: 'no STUN server answered', paths: [] },
    notes: ['The router\'s own internet address (100.72.0.1) is not public: carrier-grade NAT.'],
  });
  assert.match(out, /Not reachable/);
  assert.match(out, /no STUN server answered/);
  assert.match(out, /carrier-grade NAT/);
  const off = html({ ...base, enabled: false, rendezvous: null });
  assert.match(off, /Off/);
  assert.doesNotMatch(off, /Reachable|Router type/);
});

test('Play over the internet is a Network setting, off by default, with its STUN servers beside it', () => {
  const play = settings.find((setting) => setting.key === 'internet_play');
  assert.equal(play.category, 'network');
  assert.equal(play.default, false);
  assert.equal(play.label, 'Play over the internet');
  assert.doesNotMatch(`${play.label} ${play.description} ${play.details}`, /experimental|beta|preview/i);
  const stun = settings.find((setting) => setting.key === 'internet_play_stun_servers');
  assert.equal(stun.visibleWhen({ internet_play: false }), false);
  assert.equal(stun.visibleWhen({ internet_play: true }), true);
  assert.equal(typeof api.health.internetPlay, 'function');
});
