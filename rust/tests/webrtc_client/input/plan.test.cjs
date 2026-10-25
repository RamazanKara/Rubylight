// `node --test rust/tests/webrtc_client/input/plan.test.cjs`: the input leg's checks against an echo
// log written by hand from the script, and against the same log with one thing wrong. A check that
// cannot fail would let a broken host pass the leg.
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const plan = require('./plan.cjs');

const GERMAN = 0x0407, US = 0x0409;
const ok = checks => checks.every(c => c.ok);
const failing = checks => checks.filter(c => !c.ok).map(c => c.name);

// The low-level hook's view of a key script, with window messages and characters as Windows makes them.
function keyLog(layout, steps = plan.keySteps()) {
  const events = [];
  const generic = vk => (vk >= 0xa0 && vk <= 0xa5 ? 0x10 + ((vk - 0xa0) >> 1) : vk);
  for (const e of plan.expectedKeyEvents(steps, layout)) {
    events.push({src: 'll', ev: e.ev, vk: e.vk, scan: 0, ext: !!e.ext});
    if (e.ev === 'kd') {
      events.push({src: 'wnd', ev: 'WM_KEYDOWN', vk: generic(e.vk)});
      const letter = /^[0-9A-Z]$/.test(String.fromCharCode(e.vk));
      if (letter) events.push({src: 'wnd', ev: 'WM_CHAR', char: String.fromCharCode(e.vk).toLowerCase().charCodeAt(0)});
    }
  }
  // Shift+A types a capital, Ctrl+C types a control character.
  events.push({src: 'wnd', ev: 'WM_CHAR', char: 0x41}, {src: 'wnd', ev: 'WM_CHAR', char: 3});
  return events;
}

test('a faithful key log passes on US and QWERTZ layouts, and Y and Z swap only on QWERTZ', () => {
  assert.ok(ok(plan.checkKeys(keyLog(US), plan.keySteps(), US)), failing(plan.checkKeys(keyLog(US), plan.keySteps(), US)).join());
  assert.ok(ok(plan.checkKeys(keyLog(GERMAN), plan.keySteps(), GERMAN)));
  assert.equal(plan.vkOf('KeyY', US), 0x59);
  assert.equal(plan.vkOf('KeyY', GERMAN), 0x5a);
  assert.equal(plan.vkOf('KeyZ', GERMAN), 0x59);
  // A German layout read as a US one is a mismatch.
  assert.ok(!ok(plan.checkKeys(keyLog(GERMAN), plan.keySteps(), US)));
});

test('a wrong virtual key, a missing release and a missing key all fail', () => {
  const log = keyLog(US);
  const wrong = log.map(e => (e.src === 'll' && e.vk === 0x41 ? {...e, vk: 0x42} : e));
  assert.ok(!ok(plan.checkKeys(wrong, plan.keySteps(), US)));
  const index = log.findIndex(e => e.src === 'll' && e.ev === 'ku');
  assert.ok(!ok(plan.checkKeys(log.filter((_, i) => i !== index), plan.keySteps(), US)));
  assert.ok(!ok(plan.checkKeys(log.filter(e => !(e.src === 'll' && e.vk === 0x70)), plan.keySteps(), US)));
});

test('AltGr\'s extra left Control is ignored, any other extra key is not', () => {
  const log = keyLog(GERMAN);
  const at = log.findIndex(e => e.src === 'll' && e.vk === 0xa5 && e.ev === 'kd');
  const withCtrl = [...log.slice(0, at), {src: 'll', ev: 'kd', vk: 0xa2, scan: 0, ext: false}, ...log.slice(at)];
  assert.ok(ok(plan.checkKeys(withCtrl, plan.keySteps(), GERMAN)));
  const extra = [...log.slice(0, at), {src: 'll', ev: 'kd', vk: 0x41, scan: 0, ext: false}, ...log.slice(at)];
  assert.ok(!ok(plan.checkKeys(extra, plan.keySteps(), GERMAN)));
});

test('text must arrive as the same characters', () => {
  const wanted = [...plan.TEXTS.join('')].map(c => c.codePointAt(0));
  const log = [...wanted.flatMap(code => [{src: 'll', ev: 'kd', vk: 0xe7, scan: code}, {src: 'wnd', ev: 'WM_CHAR', char: code}])];
  assert.ok(ok(plan.checkText(log)));
  assert.ok(!ok(plan.checkText(log.filter(e => e.char !== 0x6f22 && e.scan !== 0x6f22))));
});

test('relative moves carry their fractions and compare on raw deltas', () => {
  assert.deepEqual(plan.expectedRelative(), [[10, 0], [0, 10], [-10, 0], [0, -10], [30, 0], [0, 15], [-10, -5], [1, 1]]);
  const raw = plan.expectedRelative().map(([dx, dy]) => ({src: 'raw', ev: 'mm', dx, dy, absolute: false}));
  const moves = [{src: 'll', ev: 'mm', x: 100, y: 100}, {src: 'll', ev: 'mm', x: 130, y: 120}];
  assert.ok(ok(plan.checkRelative([...raw, ...moves])));
  assert.ok(!ok(plan.checkRelative([...raw.slice(1), ...moves])));
});

test('absolute positions follow the letterbox and fail beyond the tolerance', () => {
  const screen = {name: 'D', left: 0, top: 0, width: 2880, height: 1800};
  assert.deepEqual(plan.letterbox([2880, 1800], [1920, 1080]), [96, 0, 1728, 1080]);
  assert.deepEqual(plan.expectedPixel(screen, [1920, 1080], [0, 0]), [0, 0]);
  assert.deepEqual(plan.expectedPixel(screen, [1920, 1080], [1, 1]), [2879, 1799]);
  assert.deepEqual(plan.expectedPixel(screen, [1920, 1080], [0.5, 0.5]), [1439, 899]);
  const log = plan.POSITIONS.map(p => { const [x, y] = plan.expectedPixel(screen, [1920, 1080], p); return {src: 'll', ev: 'mm', x, y}; });
  assert.ok(ok(plan.checkAbsolute(log, [screen], [1920, 1080])));
  assert.ok(ok(plan.checkAbsolute(log.map(e => ({...e, x: e.x + 3})), [screen], [1920, 1080])));
  assert.ok(!ok(plan.checkAbsolute(log.map(e => ({...e, x: e.x + 9})), [screen], [1920, 1080])));
  // Stretching to the whole screen instead of the picture is caught: the left edge is at 96 px, not 0.
  assert.ok(!ok(plan.checkAbsolute(log.map(e => ({...e, x: Math.round(e.x * 0.9)})), [screen], [1920, 1080])));
});

test('buttons: each goes down and up once in order, and a wrong name fails', () => {
  const log = plan.BUTTONS.flatMap(([, name]) => [{src: 'll', ev: 'bd', button: name}, {src: 'll', ev: 'bu', button: name}]);
  assert.ok(ok(plan.checkButtons(log)));
  // Left's release may follow the next buttons' events (the host's 10 ms deferral after absolute input).
  const late = [log[0], ...log.slice(2, 6), log[1], ...log.slice(6)];
  assert.ok(ok(plan.checkButtons(late)));
  assert.ok(!ok(plan.checkButtons(log.map(e => (e.button === 'right' ? {...e, button: 'x1'} : e)))));
  assert.ok(!ok(plan.checkButtons(log.filter((_, i) => i !== 1))));
});

test('wheel: down is negative, up positive, right positive', () => {
  const log = plan.WHEELS.flatMap(w => w.expect.map(([ev, delta]) => ({src: 'll', ev, delta})));
  assert.ok(ok(plan.checkWheel(log)));
  assert.ok(!ok(plan.checkWheel(log.map(e => ({...e, delta: -e.delta})))));
});

test('stuck keys must be released', () => {
  const log = [
    {src: 'll', ev: 'kd', vk: 0x51}, {src: 'll', ev: 'kd', vk: 0xa0}, {src: 'll', ev: 'bd', button: 'middle'},
    {src: 'll', ev: 'ku', vk: 0x51}, {src: 'll', ev: 'ku', vk: 0xa0}, {src: 'll', ev: 'bu', button: 'middle'}];
  assert.ok(ok(plan.checkStuck(log)));
  assert.deepEqual(failing(plan.checkStuck(log.filter(e => e.ev !== 'bu'))).length, 1);
  assert.equal(failing(plan.checkStuck(log.slice(0, 3))).length, 3);
});

test('gamepad mouse mode: stick deltas and clicks compare exactly', () => {
  const six = Array.from({length: 10}, () => ({src: 'raw', ev: 'mm', dx: 6, dy: 0, absolute: false}));
  assert.ok(plan.checkPadMove('right', six, [6, 0]).ok);
  assert.ok(!plan.checkPadMove('right', [...six, {src: 'raw', ev: 'mm', dx: 7, dy: 0, absolute: false}], [6, 0]).ok);
  assert.ok(!plan.checkPadMove('right', six.slice(0, 3), [6, 0]).ok);
  const clicks = plan.PAD_CLICKS.flatMap(([, name]) => [{src: 'll', ev: 'bd', button: name}, {src: 'll', ev: 'bu', button: name}]);
  assert.ok(plan.checkPadClicks(clicks).ok);
  assert.ok(!plan.checkPadClicks(clicks.slice(2)).ok);
  const notches = plan.PAD_NOTCHES.map(([, ev, delta]) => ({src: 'll', ev, delta}));
  assert.ok(plan.checkPadNotches(notches).ok);
  assert.ok(!plan.checkPadNotches(notches.reverse()).ok);
});
