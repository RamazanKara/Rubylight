import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { patchValue, normalise, describeDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { toDraft, changes, validCouchHold, COUCH_OPEN_WITH_OPTIONS } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const CouchSettings = (await server.ssrLoadModule('/src/components/library/CouchSettings.svelte')).default;
const VideoSettings = (await server.ssrLoadModule('/src/components/library/VideoSettings.svelte')).default;
const { session } = await server.ssrLoadModule('/src/lib/session.svelte.ts');
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const shipped = new Map(schema.settings.map(setting => [setting.key, setting]));
const client = extra => ({ uuid: 'u', name: 'TV', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('the couch menu has its own group in Sound & controls, with the host defaults', () => {
  const keys = ['couch_menu', 'couch_menu_open_with', 'couch_menu_controller_chord', 'couch_menu_hold_ms',
    'couch_menu_corner_hold_ms', 'couch_menu_keyboard_chord'];
  assert.deepEqual(keys.map(key => shipped.get(key)?.group), keys.map(() => 'Couch menu'));
  assert.deepEqual(keys.map(key => shipped.get(key).default),
    [true, 'controller,keyboard,touch', 'l3+r3', 1000, 1500, 'ctrl+alt+backspace']);
  assert.equal(shipped.get('couch_menu').control.kind, 'toggle');
  assert.equal(shipped.get('couch_menu').advanced, false);
});

test('the ways to open the menu are one switch each, and none stays an explicit empty list', () => {
  const setting = shipped.get('couch_menu_open_with');
  assert.deepEqual(setting.control.options.map(option => option.value), ['controller', 'keyboard', 'touch']);
  assert.deepEqual(normalise(setting, 'controller,touch'), ['controller', 'touch']);
  assert.equal(patchValue(setting, ['controller', 'touch']), 'controller,touch');
  // An empty value is dropped by the configuration file and means the default there.
  assert.equal(patchValue(setting, []), ',');
  assert.deepEqual(normalise(setting, ','), []);
  assert.match(describeDefault(setting).text, /^Controller chord, Keyboard chord, Top-left/);
});

test('night mode and headphone surround are host-wide defaults under Sound', () => {
  for (const key of ['audio_night_mode', 'audio_headphone_surround']) {
    const setting = shipped.get(key);
    assert.deepEqual([setting.category, setting.group, setting.default, setting.control.kind],
      ['audio', 'Sound', false, 'toggle'], key);
  }
});

test('a device sets sound overrides (stream audio, passthrough, sink, night mode and surround) beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.deepEqual(before.audio, {
    stream_audio: '',
    audio_passthrough: '',
    audio_night_mode: '',
    audio_headphone_surround: '',
  });
  assert.equal(before.audio_sink, '');
  assert.deepEqual(
    changes(before, {
      ...before,
      audio: { ...before.audio, stream_audio: 'off', audio_passthrough: 'on', audio_night_mode: 'on' },
      audio_sink: ' {sink-1} ',
    }),
    {
      config_overrides: {
        max_bitrate: 20000,
        stream_audio: false,
        audio_passthrough: true,
        audio_night_mode: true,
        audio_sink: '{sink-1}',
      },
    },
  );
  const stored = toDraft(
    client({
      config_overrides: {
        stream_audio: 'false',
        audio_passthrough: true,
        audio_headphone_surround: false,
        audio_sink: '{sink-hdmi}',
      },
    }),
  );
  assert.equal(stored.audio.stream_audio, 'off');
  assert.equal(stored.audio.audio_passthrough, 'on');
  assert.equal(stored.audio.audio_headphone_surround, 'off');
  assert.equal(stored.audio_sink, '{sink-hdmi}');
  for (const key of ['stream_audio', 'audio_passthrough', 'audio_headphone_surround', 'audio_sink']) {
    assert.equal(key in stored.other_overrides, false, key);
  }
  // Untouched values go back as stored without spurious config_overrides diff.
  assert.deepEqual(changes(stored, { ...stored, name: 'Cube' }), { name: 'Cube' });
  assert.deepEqual(
    changes(stored, {
      ...stored,
      audio: { ...stored.audio, stream_audio: '', audio_passthrough: '', audio_headphone_surround: '' },
      audio_sink: '',
    }),
    { config_overrides: null },
  );
});

test('a device sets the couch menu beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.deepEqual(Object.values(before.couch), ['', '', '', '', '', '']);
  const set = { couch_menu: 'off', couch_menu_open_with: 'keyboard,touch', couch_menu_controller_chord: ' select ',
    couch_menu_hold_ms: '1200', couch_menu_corner_hold_ms: '2000', couch_menu_keyboard_chord: 'ctrl+alt+home' };
  assert.deepEqual(changes(before, { ...before, couch: set }), {
    config_overrides: { max_bitrate: 20000, couch_menu: false, couch_menu_open_with: 'keyboard,touch',
      couch_menu_controller_chord: 'select', couch_menu_hold_ms: 1200, couch_menu_corner_hold_ms: 2000,
      couch_menu_keyboard_chord: 'ctrl+alt+home' },
  });
  const stored = toDraft(client({ config_overrides: { couch_menu: 'false', couch_menu_hold_ms: 1500 } }));
  assert.equal(stored.couch.couch_menu, 'off');
  assert.equal(stored.couch.couch_menu_hold_ms, '1500');
  assert.deepEqual(Object.keys(stored.other_overrides), []);
  // Untouched, they go back as stored; emptied, they leave.
  assert.deepEqual(changes(stored, { ...stored, name: 'Cube' }), { name: 'Cube' });
  assert.deepEqual(changes(stored, { ...stored, couch: { ...stored.couch, couch_menu: '', couch_menu_hold_ms: '' } }),
    { config_overrides: null });
});

test('a couch menu hold is empty or a whole number inside the host range', () => {
  for (const value of ['', '300', '3000']) assert.equal(validCouchHold('couch_menu_hold_ms', value), true, value);
  for (const value of ['299', '3001', '1.5', 'soon']) assert.equal(validCouchHold('couch_menu_hold_ms', value), false, value);
  assert.equal(validCouchHold('couch_menu_corner_hold_ms', '400'), false);
  assert.equal(validCouchHold('couch_menu_corner_hold_ms', '500'), true);
  assert.equal(COUCH_OPEN_WITH_OPTIONS.length, 7);
  assert.equal(COUCH_OPEN_WITH_OPTIONS.at(-1).value, 'controller,keyboard,touch');
});

test('the device editor has Sound and Couch menu groups that open on a saved value', async () => {
  const { readFile } = await import('node:fs/promises');
  const prevMetadata = session.metadata;
  session.metadata = {
    audio_sinks: [{ id: '{sink-avr}', name: 'HDMI AVR', description: 'HDMI AVR', adapter: 'GPU', default: true, virtual_sink: false }],
  };
  try {
    const body = extra => render(DeviceEditor, { props: { client: client(extra), sleepUnavailable: null } }).body;
    const section = (html, name) => new RegExp(`<details class="override[^>]*><summary[^>]*>${name}<`).exec(html)[0];
    for (const label of [
      'Stream audio', 'PCM and Dolby Digital audio', 'Audio device', 'HDMI AVR · default',
      'Night mode', 'Headphone surround', 'Open the couch menu with', 'Controller chord hold', 'Corner hold', 'Keyboard chord',
    ]) {
      assert.match(body({}), new RegExp(label), label);
    }
    assert.doesNotMatch(section(body({}), 'Couch menu'), /\sopen/);
    assert.match(section(body({ config_overrides: { couch_menu: false } }), 'Couch menu'), /\sopen/);
    assert.doesNotMatch(section(body({ config_overrides: { couch_menu: false } }), 'Sound'), /\sopen/);
    assert.match(section(body({ config_overrides: { audio_night_mode: true } }), 'Sound'), /\sopen/);
    assert.match(section(body({ config_overrides: { stream_audio: false } }), 'Sound'), /\sopen/);
    assert.match(section(body({ config_overrides: { audio_sink: '{sink-avr}' } }), 'Sound'), /\sopen/);
    assert.match(section(body({ config_overrides: { couch_menu_hold_ms: 'soon' } }), 'Couch menu'), /\sopen/);
    const editorSource = await readFile(new URL('./src/components/devices/DeviceEditor.svelte', import.meta.url), 'utf8');
    assert.match(editorSource, /disabled=\{!changed \|\| nameMissing \|\| modeInvalid \|\| delayInvalid \|\| couchInvalid \|\| cascadeInvalid \|\| disabled\}/);
  } finally {
    session.metadata = prevMetadata;
  }
});

test('an app sets native pen and touch as a tristate override in VideoSettings', () => {
  const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
  const draft = new AppDraft(app({}));
  const html = render(VideoSettings, { props: { draft } }).body;
  assert.match(html, /Native pen and touch/);
  assert.equal(draft.tristate('native-pen-touch'), '');
  draft.setTristate('native-pen-touch', 'off');
  assert.equal(draft.record()['native-pen-touch'], false);
  draft.setTristate('native-pen-touch', 'on');
  assert.equal(draft.record()['native-pen-touch'], true);
  draft.setTristate('native-pen-touch', '');
  assert.ok(!('native-pen-touch' in draft.record()));
});

test('an app sets night mode, headphone surround and the couch menu for itself', () => {
  const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
  const draft = new AppDraft(app({}));
  const html = render(CouchSettings, { props: { draft } }).body;
  for (const label of ['Night mode', 'Headphone surround', 'Couch menu', 'Open the couch menu with', 'Controller chord',
    'Controller chord hold', 'Corner hold', 'Keyboard chord']) {
    assert.match(html, new RegExp(label), label);
  }
  draft.setTristate('audio-night-mode', 'on');
  draft.setTristate('couch-menu', 'off');
  draft.setText('couch-menu-open-with', 'keyboard');
  draft.setText('couch-menu-keyboard-chord', 'ctrl+alt+end');
  draft.setNumber('couch-menu-hold-ms', 200);
  assert.equal(draft.invalid, true);
  draft.setNumber('couch-menu-hold-ms', 1200);
  assert.equal(draft.invalid, false);
  assert.deepEqual(
    Object.fromEntries(Object.entries(draft.record()).filter(([key]) => key.startsWith('couch-') || key.startsWith('audio-'))),
    { 'audio-night-mode': true, 'couch-menu': false, 'couch-menu-open-with': 'keyboard',
      'couch-menu-keyboard-chord': 'ctrl+alt+end', 'couch-menu-hold-ms': 1200 },
  );
  draft.setTristate('audio-night-mode', '');
  assert.ok(!('audio-night-mode' in draft.record()));
});
