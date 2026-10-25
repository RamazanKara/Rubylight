// The keyboard and mouse script for the WebRTC input leg and the checks that compare the echo
// window's log with it (docs/webrtc-design.md, "Testing"). Pure functions: no I/O, so they run
// under `node --test` against a log that was written by hand.
//
// The expected virtual keys, buttons and wheel signs are written out here from the Win32 and DOM
// definitions and not read from the host's tables, so a wrong table entry fails the leg.
'use strict';

const SENTINEL = {code: 'F24', vk: 0x87};
const VK = {
  Enter: 0x0d, Tab: 0x09, Space: 0x20, ShiftLeft: 0xa0, ShiftRight: 0xa1, ControlLeft: 0xa2, ControlRight: 0xa3,
  AltLeft: 0xa4, AltRight: 0xa5, ArrowLeft: 0x25, ArrowUp: 0x26, ArrowRight: 0x27, ArrowDown: 0x28,
};
const expectedKey = {};
for (let i = 0; i < 26; i++) expectedKey['Key' + String.fromCharCode(65 + i)] = {vk: 0x41 + i};
for (let i = 0; i < 10; i++) expectedKey['Digit' + i] = {vk: 0x30 + i};
for (let i = 1; i <= 24; i++) expectedKey['F' + i] = {vk: 0x6f + i};
for (const [code, vk] of Object.entries(VK)) expectedKey[code] = {vk};
expectedKey.NumpadEnter = {vk: 0x0d, ext: true};
expectedKey.Enter = {vk: 0x0d, ext: false};

const key = (code, down) => ({type: 'key', code, down});
const tap = code => [key(code, true), 8, key(code, false), 8];
const sentinel = () => tap(SENTINEL.code);

// ---- the script -----------------------------------------------------------------------------------------

// Letters, digits, modifiers, F-keys, arrows, Enter; then a chord. Alt comes last: it is the only
// key that can leave the window in menu mode (the echo window swallows that, but be careful anyway).
const KEY_CODES = [
  ...Array.from({length: 26}, (_, i) => 'Key' + String.fromCharCode(65 + i)),
  ...Array.from({length: 10}, (_, i) => 'Digit' + i),
  'ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight',
  ...Array.from({length: 12}, (_, i) => 'F' + (i + 1)),
  'ArrowLeft', 'ArrowUp', 'ArrowRight', 'ArrowDown', 'Enter', 'NumpadEnter', 'Space', 'Tab',
  'AltLeft', 'AltRight',
];

function keySteps() {
  const steps = [];
  for (const code of KEY_CODES) steps.push(...tap(code));
  // Shift+A types a capital; Ctrl+C is a chord whose letter does not type.
  steps.push(key('ShiftLeft', true), 8, ...tap('KeyA'), key('ShiftLeft', false), 8);
  steps.push(key('ControlLeft', true), 8, ...tap('KeyC'), key('ControlLeft', false), 8);
  return steps;
}

// Keys are placed by physical position (a `KeyboardEvent.code` names a position, and the host sends
// the scan code of the US key there), so the virtual key a layout reports for a position can differ:
// the Y and Z positions swap on a QWERTZ layout. `layout` is the primary language id of the laptop's
// keyboard layout, as the echo window reports it.
const QWERTZ = new Set([0x07, 0x05, 0x0e, 0x1b, 0x24, 0x1a]);
const primaryLanguage = layout => (layout || 0x0409) & 0x3ff;
function vkOf(code, layout) {
  if (QWERTZ.has(primaryLanguage(layout))) {
    if (code === 'KeyY') return expectedKey.KeyZ.vk;
    if (code === 'KeyZ') return expectedKey.KeyY.vk;
  }
  return expectedKey[code].vk;
}

// What the low-level hook should see for the key script, in order.
function expectedKeyEvents(steps, layout) {
  return steps.filter(s => typeof s !== 'number').map(s => {
    const e = expectedKey[s.code];
    return {ev: s.down ? 'kd' : 'ku', vk: vkOf(s.code, layout), code: s.code, ext: e.ext};
  });
}

const TEXTS = ['é', 'ü', '漢', 'aé漢€'];
const textSteps = () => TEXTS.flatMap(text => [{type: 'text', text}, 20]);

const RELATIVE = [[10, 0], [0, 10], [-10, 0], [0, -10], [30, 0], [0, 15], [-10, -5], [0.4, 0.4], [0.4, 0.4], [0.4, 0.4]];
const relativeSteps = () => RELATIVE.flatMap(([dx, dy]) => [{type: 'mouse-move', dx, dy}, 12]);

// Fractions of the video: corners, centre and two inner points.
const POSITIONS = [[0, 0], [1, 0], [1, 1], [0, 1], [0.5, 0.5], [0.25, 0.75], [0.75, 0.25]];
const absoluteSteps = () => POSITIONS.flatMap(([x, y]) => [{type: 'mouse-position', x, y}, 40]);
const centreStep = () => [{type: 'mouse-position', x: 0.5, y: 0.5}, 60];

// DOM button numbers 0 to 4 are left, middle, right, back, forward.
const BUTTONS = [[0, 'left'], [1, 'middle'], [2, 'right'], [3, 'x1'], [4, 'x2']];
const buttonSteps = () => BUTTONS.flatMap(([button]) => [{type: 'mouse-button', button, down: true}, 60, {type: 'mouse-button', button, down: false}, 60]);

// DOM deltas are positive down and right; Windows wheel deltas are positive up and right.
const WHEELS = [
  {dx: 0, dy: 120, expect: [['wheel', -120]]},
  {dx: 0, dy: -120, expect: [['wheel', 120]]},
  {dx: 0, dy: 240, expect: [['wheel', -240]]},
  {dx: 120, dy: 0, expect: [['hwheel', 120]]},
  {dx: -120, dy: 0, expect: [['hwheel', -120]]},
  {dx: 120, dy: 120, expect: [['wheel', -120], ['hwheel', 120]]},
];
const wheelSteps = () => WHEELS.flatMap(({dx, dy}) => [{type: 'wheel', dx, dy}, 30]);

// A key, a modifier and a button held when the channel closes.
const STUCK = {keys: ['KeyQ', 'ShiftLeft'], button: 1, buttonName: 'middle'};
const stuckSteps = () => [...STUCK.keys.map(code => key(code, true)), 8, {type: 'mouse-button', button: STUCK.button, down: true}, 30];

// ---- reading the echo log -------------------------------------------------------------------------------

const ll = (events, ...kinds) => events.filter(e => e.src === 'll' && kinds.includes(e.ev));
const pass = (name, detail) => ({name, ok: true, ...detail});
const fail = (name, detail) => ({name, ok: false, ...detail});

function firstDifference(expected, seen, show) {
  const n = Math.max(expected.length, seen.length);
  for (let i = 0; i < n; i++) if (JSON.stringify(show(expected[i])) !== JSON.stringify(show(seen[i]))) return {index: i, expected: expected[i] && show(expected[i]), seen: seen[i] && show(seen[i])};
  return null;
}

// Right Alt is AltGr on layouts that have it, and Windows adds a left Control around it.
function withoutAltGrControl(seen) {
  return seen.filter((e, i) => !(e.vk === 0xa2 && ((seen[i - 1] || {}).vk === 0xa5 || (seen[i + 1] || {}).vk === 0xa5)));
}

function checkKeys(events, steps = keySteps(), layout) {
  const expected = expectedKeyEvents(steps, layout);
  const seen = withoutAltGrControl(ll(events, 'kd', 'ku'));
  const result = [];
  const diff = firstDifference(expected, seen, e => e && [e.ev, e.vk]);
  result.push(diff ? fail('keys map to the right virtual keys', {expected: expected.length, seen: seen.length, ...diff})
    : pass('keys map to the right virtual keys', {keys: expected.length / 2}));
  // Keypad Enter is told apart from Enter by the extended bit.
  const extended = expected.map((e, i) => ({e, s: seen[i]})).filter(x => x.e.code === 'NumpadEnter' || x.e.code === 'Enter');
  result.push(extended.every(x => x.s && x.s.ext === !!x.e.ext) ? pass('Enter and keypad Enter differ by the extended bit', {})
    : fail('Enter and keypad Enter differ by the extended bit', {seen: extended.map(x => [x.e.code, x.s && x.s.ext])}));
  // The window sees them as WM_KEYDOWN/UP in the same order (modifiers and Alt may arrive as SYS messages).
  const downs = events.filter(e => e.src === 'wnd' && /KEYDOWN$/.test(e.ev)).map(e => e.vk);
  // Window messages name Shift, Control and Alt without a side.
  const generic = vk => (vk >= 0xa0 && vk <= 0xa5 ? 0x10 + ((vk - 0xa0) >> 1) : vk);
  const wanted = expected.filter(e => e.ev === 'kd').map(e => generic(e.vk));
  let at = 0;
  for (const vk of downs) if (at < wanted.length && vk === wanted[at]) at++;
  result.push(at === wanted.length ? pass('the focused window receives WM_KEYDOWN for each key', {})
    : fail('the focused window receives WM_KEYDOWN for each key', {matched: at, wanted: wanted.length, firstMissing: wanted[at]}));
  // Typing: lower-case letters, the digits, a capital from Shift+A, and no character from Ctrl+C.
  const chars = events.filter(e => e.src === 'wnd' && e.ev === 'WM_CHAR').map(e => e.char);
  // The characters of the letter and digit keys in script order: the position's virtual key, lower case.
  const typed = KEY_CODES.filter(code => /^(Key|Digit)/.test(code)).map(code => String.fromCharCode(vkOf(code, layout)).toLowerCase().charCodeAt(0));
  let k = 0;
  for (const c of chars) if (k < typed.length && c === typed[k]) k++;
  result.push(k === typed.length ? pass('letters and digits type their characters', {}) : fail('letters and digits type their characters', {typed: k, of: typed.length, chars}));
  result.push(chars.includes(0x41) ? pass('Shift+A types a capital A', {}) : fail('Shift+A types a capital A', {chars}));
  result.push(!chars.includes(0x03 + 0x40) ? pass('Ctrl+C types no letter', {}) : fail('Ctrl+C types no letter', {chars}));
  return result;
}

function checkText(events) {
  const wanted = [...TEXTS.join('')].flatMap(ch => [...ch].map(c => c.codePointAt(0)));
  const chars = events.filter(e => e.src === 'wnd' && e.ev === 'WM_CHAR').map(e => e.char);
  const packets = ll(events, 'kd').filter(e => e.vk === 0xe7).map(e => e.scan);
  return [
    JSON.stringify(chars) === JSON.stringify(wanted) ? pass('text arrives as the same characters', {chars: chars.map(c => String.fromCodePoint(c)).join('')})
      : fail('text arrives as the same characters', {wanted: wanted.map(c => String.fromCodePoint(c)).join(''), seen: chars.map(c => String.fromCodePoint(c)).join(''), wantedCodes: wanted, seenCodes: chars}),
    JSON.stringify(packets) === JSON.stringify(wanted) ? pass('the injected packets carry the same UTF-16 units', {})
      : fail('the injected packets carry the same UTF-16 units', {wanted, seen: packets}),
  ];
}

// The whole pixels the host takes out of the relative script, remainders carried like the host does.
function expectedRelative() {
  const out = [];
  let rx = 0, ry = 0;
  for (const [dx, dy] of RELATIVE) {
    const tx = rx + dx, ty = ry + dy;
    const wx = Math.trunc(tx), wy = Math.trunc(ty);
    rx = tx - wx; ry = ty - wy;
    if (wx || wy) out.push([wx, wy]);
  }
  return out;
}

function checkRelative(events) {
  const raw = events.filter(e => e.src === 'raw' && !e.absolute).map(e => [e.dx, e.dy]);
  const expected = expectedRelative();
  const result = [JSON.stringify(raw) === JSON.stringify(expected) ? pass('relative moves arrive as the same deltas', {moves: expected.length})
    : fail('relative moves arrive as the same deltas', {expected, seen: raw})];
  const moves = ll(events, 'mm');
  if (moves.length >= 2) {
    const [a, b] = [moves[0], moves[moves.length - 1]];
    const net = expected.reduce((n, [x, y]) => [n[0] + x, n[1] + y], [0, 0]);
    const sign = Math.sign;
    result.push(sign(b.x - a.x) === sign(net[0]) && sign(b.y - a.y) === sign(net[1])
      ? pass('the pointer ends up in the direction of the net motion', {script: net, pointer: [b.x - a.x, b.y - a.y]})
      : fail('the pointer ends up in the direction of the net motion', {script: net, pointer: [b.x - a.x, b.y - a.y]}));
  } else result.push(fail('the pointer moved', {moves: moves.length}));
  return result;
}

// The picture of a `display`-shaped source in a `stream`-shaped frame: core's display_policy::letterbox.
function letterbox(display, stream) {
  const full = [0, 0, stream[0], stream[1]];
  if (!display[0] || !display[1] || (display[0] === stream[0] && display[1] === stream[1])) return full;
  const scale = Math.min(stream[0] / display[0], stream[1] / display[1]);
  const w = display[0] * scale, h = display[1] * scale;
  if (Math.abs(w - stream[0]) < 1 && Math.abs(h - stream[1]) < 1) return full;
  const even = (value, limit) => Math.min(Math.max(Math.round(value) & ~1, 2), limit);
  const [pw, ph] = [even(w, stream[0]), even(h, stream[1])];
  return [((stream[0] - pw) / 2) & ~1, ((stream[1] - ph) / 2) & ~1, pw, ph];
}

// Where a fraction of the video lands on `screen` ({left, top, width, height}), as the injector maps it.
function expectedPixel(screen, stream, [fx, fy]) {
  const clamp = v => Math.min(Math.max(v, 0), 1);
  const [width, height] = stream;
  // The client's x is rounded to the stream's pixel grid before it leaves the host's input channel.
  const px = Math.round(clamp(fx) * width), py = Math.round(clamp(fy) * height);
  let [ux, uy] = [px / width, py / height];
  const [ox, oy, pw, ph] = letterbox([screen.width, screen.height], stream);
  if (ox !== 0 || oy !== 0 || pw !== width || ph !== height) {
    ux = clamp((ux * width - ox) / Math.max(pw, 1));
    uy = clamp((uy * height - oy) / Math.max(ph, 1));
  }
  return [screen.left + Math.trunc(ux * (screen.width - 1)), screen.top + Math.trunc(uy * (screen.height - 1))];
}

// The display whose expected positions the pointer actually followed, within `tolerance` pixels.
function checkAbsolute(events, screens, stream, tolerance = 3) {
  const moves = ll(events, 'mm');
  const tries = screens.map(screen => {
    const expected = POSITIONS.map(p => expectedPixel(screen, stream, p));
    // One position per message, in order: take the first move at or after the last match that is close.
    let from = 0;
    const found = expected.map(([x, y]) => {
      for (let i = from; i < moves.length; i++) {
        if (Math.abs(moves[i].x - x) <= tolerance && Math.abs(moves[i].y - y) <= tolerance) { from = i + 1; return {x: moves[i].x, y: moves[i].y, dx: moves[i].x - x, dy: moves[i].y - y}; }
      }
      return null;
    });
    return {screen: screen.name, expected, found, ok: found.every(Boolean)};
  });
  const best = tries.find(t => t.ok);
  if (best) return [pass('absolute positions land within a few pixels', {tolerance, screen: best.screen, stream, expected: best.expected, found: best.found})];
  const closest = tries.sort((a, b) => b.found.filter(Boolean).length - a.found.filter(Boolean).length)[0];
  return [fail('absolute positions land within a few pixels', {tolerance, stream, moves: moves.map(m => [m.x, m.y]), tried: closest})];
}

// Each button goes down and up once, in the order of the script. Different buttons may interleave:
// after absolute input the host holds a left release back 10 ms, as Sunshine does, so a later
// button's events can overtake it.
function checkButtons(events) {
  const name = 'button numbers 0-4 are left, middle, right, back, forward';
  const all = ll(events, 'bd', 'bu').map(e => [e.ev, e.button]);
  const downs = all.filter(e => e[0] === 'bd').map(e => e[1]);
  const wanted = BUTTONS.map(b => b[1]);
  for (const [, button] of BUTTONS) {
    const mine = all.filter(e => e[1] === button).map(e => e[0]);
    if (JSON.stringify(mine) !== JSON.stringify(['bd', 'bu'])) return [fail(name, {button, seen: mine, all})];
  }
  return JSON.stringify(downs) === JSON.stringify(wanted) ? [pass(name, {order: all.map(e => e.join(' ')).join(', ')})] : [fail(name, {downs, wanted, all})];
}

function checkWheel(events) {
  const seen = ll(events, 'wheel', 'hwheel').map(e => [e.ev, e.delta]);
  const expected = WHEELS.flatMap(w => w.expect);
  const diff = firstDifference(expected, seen, e => e);
  return [diff ? fail('wheel notches have the right direction and size', {seen, ...diff}) : pass('wheel notches have the right direction and size', {notches: expected.length})];
}

// After the channel closes with Q, Shift and the middle button held, the echo must see each released.
function checkStuck(events) {
  const result = [];
  for (const code of STUCK.keys) {
    const vk = expectedKey[code].vk;
    const mine = ll(events, 'kd', 'ku').filter(e => e.vk === vk);
    const last = mine[mine.length - 1];
    result.push(mine.length >= 2 && mine[0].ev === 'kd' && last.ev === 'ku' ? pass(`${code} held at close is released`, {})
      : fail(`${code} held at close is released`, {seen: mine.map(e => e.ev)}));
  }
  const buttons = ll(events, 'bd', 'bu').filter(e => e.button === STUCK.buttonName);
  result.push(buttons.length >= 2 && buttons[0].ev === 'bd' && buttons[buttons.length - 1].ev === 'bu' ? pass(`the ${STUCK.buttonName} button held at close is released`, {})
    : fail(`the ${STUCK.buttonName} button held at close is released`, {seen: buttons.map(e => e.ev)}));
  return result;
}

// ---- gamepad mouse mode ---------------------------------------------------------------------------------

// Gamepad API button bits (core/src/webrtc/gamepad.rs `button`).
const PAD = {A: 1, B: 2, X: 4, Y: 8, LB: 16, RB: 32, START: 128, DPAD_UP: 1024, DPAD_DOWN: 2048, DPAD_LEFT: 4096, DPAD_RIGHT: 8192};
// What each pad button does in mouse mode, and the stick motion per 50 ms tick at +-0.5: (0.5 * 4)^3 - 2.
const PAD_CLICKS = [['A', 'left'], ['B', 'right'], ['X', 'middle'], ['LB', 'x1'], ['RB', 'x2']];
const PAD_NOTCHES = [['DPAD_UP', 'wheel', 120], ['DPAD_DOWN', 'wheel', -120], ['DPAD_RIGHT', 'hwheel', 120], ['DPAD_LEFT', 'hwheel', -120]];
const STICK_STEP = 6;
const PAD_MOVES = [
  {name: 'right', state: {leftStickX: 0.5}, delta: [STICK_STEP, 0]},
  {name: 'left', state: {leftStickX: -0.5}, delta: [-STICK_STEP, 0]},
  {name: 'down', state: {leftStickY: 0.5}, delta: [0, STICK_STEP]},
  {name: 'up', state: {leftStickY: -0.5}, delta: [0, -STICK_STEP]},
];

const rawMoves = events => events.filter(e => e.src === 'raw' && !e.absolute).map(e => [e.dx, e.dy]);

function checkPadMove(name, events, delta, minimum = 8) {
  const moves = rawMoves(events);
  const wrong = moves.filter(m => m[0] !== delta[0] || m[1] !== delta[1]);
  return moves.length >= minimum && !wrong.length
    ? pass(`mouse mode: the stick ${name} moves ${JSON.stringify(delta)} px per tick`, {ticks: moves.length})
    : fail(`mouse mode: the stick ${name} moves ${JSON.stringify(delta)} px per tick`, {ticks: moves.length, minimum, wrong: wrong.slice(0, 6)});
}

function checkPadClicks(events, list = PAD_CLICKS) {
  const seen = ll(events, 'bd', 'bu').map(e => [e.ev, e.button]);
  const expected = list.flatMap(([, name]) => [['bd', name], ['bu', name]]);
  const diff = firstDifference(expected, seen, e => e);
  const name = 'mouse mode: ' + list.map(([b, n]) => `${b} is ${n}`).join(', ');
  return diff ? fail(name, {seen, ...diff}) : pass(name, {});
}

function checkPadNotches(events) {
  const seen = ll(events, 'wheel', 'hwheel').map(e => [e.ev, e.delta]);
  const expected = PAD_NOTCHES.map(([, ev, delta]) => [ev, delta]);
  const diff = firstDifference(expected, seen, e => e);
  return diff ? fail('mouse mode: the d-pad scrolls one notch (up, down, right, left)', {seen, ...diff}) : pass('mouse mode: the d-pad scrolls one notch (up, down, right, left)', {});
}

module.exports = {
  PAD, PAD_CLICKS, PAD_NOTCHES, PAD_MOVES, STICK_STEP, rawMoves, checkPadMove, checkPadClicks, checkPadNotches,
  SENTINEL, VK, KEY_CODES, vkOf, primaryLanguage, TEXTS, RELATIVE, POSITIONS, BUTTONS, WHEELS, STUCK,
  key, tap, sentinel, keySteps, textSteps, relativeSteps, absoluteSteps, centreStep, buttonSteps, wheelSteps, stuckSteps,
  expectedKeyEvents, expectedRelative, expectedPixel, letterbox,
  checkKeys, checkText, checkRelative, checkAbsolute, checkButtons, checkWheel, checkStuck, ll, pass, fail,
};
