import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { toDraft, changes, savingEndsStream } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { byClient, usbId, usbState, showUsb } = await server.ssrLoadModule('/src/components/devices/usb.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const UsbDevices = (await server.ssrLoadModule('/src/components/devices/UsbDevices.svelte')).default;
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });
const device = extra => ({
  id: 1, client: 'Phone', name: 'Flash drive', vid: 0x0781, pid: 0x5581, class: 8, speed: 3, busid: '1-2',
  state: 'attached', port: 2, since_unix_ms: 0, error: null, ...extra,
});

test('USB devices are off unless the device holds a JSON true, as the host reads it', () => {
  assert.equal(toDraft(client({})).allow_usb, false);
  // No permission bit or Host control implies it.
  assert.equal(toDraft(client({ perm: 0xffffffff, host_control: true })).allow_usb, false);
  assert.equal(toDraft(client({ allow_usb: true })).allow_usb, true);
  for (const value of [false, null, 1, '1', 'true', [true]]) {
    assert.equal(toDraft(client({ allow_usb: value })).allow_usb, false, String(value));
  }
});

test('the USB grant round-trips: true to grant, null to revoke, and revoking ends a stream', () => {
  const off = toDraft(client({}));
  const on = { ...off, allow_usb: true };
  assert.deepEqual(changes(off, on), { allow_usb: true });
  assert.deepEqual(changes(on, off), { allow_usb: null });
  const stored = toDraft(client({ allow_usb: true }));
  assert.deepEqual(stored, on);
  assert.deepEqual(changes(stored, { ...stored, name: 'Tablet' }), { name: 'Tablet' });
  // The host disconnects a device whose grant is taken away (web/clients.rs), not one that gets it.
  assert.equal(savingEndsStream(true, off, on), false);
  assert.equal(savingEndsStream(true, on, off), true);
  assert.equal(savingEndsStream(false, on, off), false);
});

test('attachments group by device, in name order', () => {
  const list = [
    device({ id: 3, client: 'Tablet', name: 'Gamepad' }),
    device({ id: 2, name: 'Keyboard' }),
    device({ id: 1, name: 'Flash drive' }),
  ];
  assert.deepEqual(
    byClient(list).map(group => [group.client, group.devices.map(d => d.id)]),
    [['Phone', [1, 2]], ['Tablet', [3]]],
  );
  assert.deepEqual(byClient([]), []);
});

test('each state has a label, and a state a newer host adds is shown as it is', () => {
  assert.equal(usbId({ vid: 0x46d, pid: 0xc52b }), '046d:c52b');
  assert.deepEqual(usbState(device({})), { label: 'Attached on port 2', tone: 'ok' });
  assert.equal(usbState(device({ port: null })).label, 'Attached');
  assert.equal(usbState(device({ state: 'waiting', port: null })).label, 'Waiting for the device');
  assert.equal(usbState(device({ state: 'opening', port: null })).label, 'Connecting');
  assert.equal(usbState(device({ state: 'detaching' })).tone, 'warn');
  assert.deepEqual(usbState(device({ state: 'detach_failed' })), { label: 'Detach failed', tone: 'danger' });
  assert.equal(usbState(device({ state: 'resetting' })).label, 'resetting');
});

test('the panel shows once a device may forward or something is attached', () => {
  assert.equal(showUsb(null, []), false);
  assert.equal(showUsb([client({})], []), false);
  assert.equal(showUsb([client({ allow_usb: true })], []), true);
  assert.equal(showUsb([client({})], [device({})]), true);
});

test('the panel lists each device under its client, with a failed detach and its reason', () => {
  const html = render(UsbDevices, {
    props: {
      devices: [device({}), device({ id: 2, client: 'Tablet', name: 'Gamepad', state: 'detach_failed', error: 'usbip detach failed' })],
      ondetach: () => {},
    },
  }).body;
  assert.doesNotMatch(html, /Experimental/);
  assert.match(html, /<h3[^>]*>Phone<\/h3>[\s\S]*Flash drive[\s\S]*0781:5581 · 1-2[\s\S]*Attached on port 2/);
  assert.match(html, /<h3[^>]*>Tablet<\/h3>[\s\S]*Gamepad[\s\S]*usbip detach failed[\s\S]*Detach failed[\s\S]*Detach again/);
  const empty = render(UsbDevices, { props: { devices: [], error: 'timed out', ondetach: () => {} } }).body;
  assert.match(empty, /No USB devices are attached/);
  assert.match(empty, /role="alert"[^>]*>Could not load USB devices: timed out/);
});
