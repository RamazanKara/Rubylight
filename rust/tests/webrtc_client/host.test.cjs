// Run with: node --test rust/tests/webrtc_client/host.test.cjs (Windows).
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {test} = require('node:test');
const {execFileSync} = require('node:child_process');
const host = require('./host.cjs');

const STAGING = 'C:\\tests\\webrtc-host';
const LAUNCHER = 'C:\\ProgramData\\ButterpolloRelease\\system_launch.exe';
const LAUNCH = `${LAUNCHER} ${STAGING}\\butterpollo.exe --config-dir C:\\tests\\webrtc-system --port 48140 --bind 127.0.0.1 --no-tray`;
const create = command => ['/Create', '/TN', 'RubylightWebrtcE2eHost', '/TR', command, '/SC', 'ONCE', '/ST', '23:59', '/RU', 'SYSTEM', '/RL', 'HIGHEST', '/F'];

const runScript = (dir, categories) => execFileSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass',
  '-File', path.join(__dirname, 'elevated_tasks.ps1'), '-Dir', dir, '-DryRun', '-Categories', categories.join(',')], {stdio: 'pipe'});

// Runs the elevated script's checks on `steps` without running anything, and returns its answer.
// `categories` are the connected networks' categories it sees.
function dryRun(steps, categories = ['Private']) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'rl-elevate-'));
  try {
    fs.writeFileSync(path.join(dir, 'request.json'), JSON.stringify({id: 'test', steps}));
    runScript(dir, categories);
    assert(!fs.existsSync(path.join(dir, 'request.json')), 'the request was not spent');
    return JSON.parse(fs.readFileSync(path.join(dir, 'result.json'), 'utf8').replace(/^\uFEFF/, ''));
  } finally {
    fs.rmSync(dir, {recursive: true, force: true});
  }
}

test('the elevated script runs the harness steps and refuses anything else', () => {
  const cases = [
    [create(LAUNCH), 0],
    [['/Run', '/TN', 'RubylightWebrtcE2eHost'], 0],
    [['/End', '/TN', 'RubylightWebrtcE2eHost'], 0],
    [['/Delete', '/TN', 'RubylightWebrtcE2eHost', '/F'], 0],
    // The installed system_launch may start only the staged host, with plain arguments.
    [create(`${LAUNCHER} C:\\Windows\\System32\\cmd.exe /c whoami`), 99],
    [create(`${LAUNCHER} ${STAGING}\\other.exe`), 99],
    [create(`${LAUNCHER} ${STAGING}\\butterpollo.exe & calc.exe`), 99],
    [create(`${LAUNCHER} ${STAGING}\\butterpollo.exe "--config-dir" x`), 99],
    [create(`${LAUNCHER} ${STAGING}\\butterpollo.exe.cmd`), 99],
    [create(`C:\\Windows\\System32\\cmd.exe /c ${LAUNCHER} ${STAGING}\\butterpollo.exe`), 99],
    // A launcher in the user's staging folder could be replaced.
    [create(LAUNCH.replace(LAUNCHER, `${STAGING}\\system_launch.exe`)), 99],
    // No control characters, and no backslash that could escape what follows.
    [create(`${LAUNCH}\n`), 99],
    [create(`${LAUNCH}\ncalc.exe`), 99],
    [create(`${LAUNCH} x\u0000y`), 99],
    [create(`${LAUNCH} x\ty`), 99],
    [create(`${LAUNCH} C:\\tests\\`), 99],
    [create(`${LAUNCH} C:\\tests\\x`), 0],
    // Only the exact one-shot SYSTEM task, under the harness's own name.
    [create(LAUNCH).map(a => a === 'SYSTEM' ? 'someone' : a), 99],
    [[...create(LAUNCH), '/XML', 'C:\\task.xml'], 99],
    [create(LAUNCH).map(a => a === 'RubylightWebrtcE2eHost' ? 'ButterpolloReleaseElevated' : a), 99],
    [['/Run', '/TN', 'ButterpolloReleaseElevated'], 99],
    [['/Delete', '/TN', 'RubylightWebrtcE2eHost'], 99],
    [['/Change', '/TN', 'RubylightWebrtcE2eHost', '/RU', 'SYSTEM'], 99],
    [[], 99],
  ];
  const answer = dryRun([...cases.map(([args]) => ({args})), {firewall: true}, {firewall: true, remove: true}]);
  assert.equal(answer.id, 'test');
  const results = [].concat(answer.results);
  cases.forEach(([args, exit], i) => assert.equal(results[i].exit, exit, `${args.join(' ')}: ${results[i].output}`));
  assert.equal(results[cases.length].output, 'firewall rule created');
  assert.equal(results[cases.length + 1].exit, 0);
});

test('the firewall rule is opened only while every connected network is Private', () => {
  for (const categories of [['Public'], ['Private', 'Public'], ['DomainAuthenticated']]) {
    const [open, close] = dryRun([{firewall: true}, {firewall: true, remove: true}], categories).results;
    assert.equal(open.exit, 1, categories.join());
    assert.match(open.output, /Private networks only.*is (Public|DomainAuthenticated).*Set-NetConnectionProfile/);
    // Removing it needs no check.
    assert.equal(close.exit, 0);
  }
  assert.equal(dryRun([{firewall: true}], ['Private', 'Private']).results[0].output, 'firewall rule created');
});

test('the elevated script refuses a request folder, request or answer that is a link', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'rl-links-'));
  const real = path.join(base, 'real'), linked = path.join(base, 'linked');
  const request = path.join(real, 'request.json'), answer = path.join(real, 'result.json');
  // Junctions need no privilege, so a user can make them.
  const junction = (target, at) => { fs.mkdirSync(target, {recursive: true}); fs.symlinkSync(target, at, 'junction'); };
  const unlink = at => { try { fs.unlinkSync(at); } catch {} };
  try {
    fs.mkdirSync(real);
    fs.writeFileSync(request, JSON.stringify({id: 'test', steps: []}));
    junction(real, linked);
    assert.throws(() => runScript(linked, ['Private']), /linked is a link or junction/);
    assert(fs.existsSync(request), 'a request behind a junction was read');
    junction(path.join(base, 'elsewhere'), answer);
    assert.throws(() => runScript(real, ['Private']), /result\.json is a link or junction/);
    assert(fs.existsSync(request), 'the request was read before the answer was checked');
    unlink(answer);
    fs.renameSync(request, path.join(base, 'moved.json'));
    junction(path.join(base, 'elsewhere'), request);
    assert.throws(() => runScript(real, ['Private']), /request\.json is a link or junction/);
  } finally {
    for (const at of [linked, answer, request]) unlink(at);
    fs.rmSync(base, {recursive: true, force: true});
  }
});

test('the elevation task serves the harness only while its scripts are the checkout\'s', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'rl-elevation-'));
  try {
    const installed = path.join(dir, 'installed'), checkout = path.join(dir, 'checkout');
    fs.mkdirSync(installed); fs.mkdirSync(checkout);
    const scripts = {'a.ps1': path.join(checkout, 'a.ps1'), 'b.ps1': path.join(checkout, 'b.ps1')};
    fs.writeFileSync(scripts['a.ps1'], 'one\n'); fs.writeFileSync(scripts['b.ps1'], 'two\n');
    fs.writeFileSync(path.join(installed, 'a.ps1'), 'one\n');
    // b.ps1 is not installed yet.
    assert.match(host.elevationProblem({installed, scripts, registered: true}), /out of date \(b\.ps1\).*elevation\.ps1/);
    fs.writeFileSync(path.join(installed, 'b.ps1'), 'two, older\n');
    assert.match(host.elevationProblem({installed, scripts, registered: true}), /\(b\.ps1\)/);
    fs.writeFileSync(path.join(installed, 'b.ps1'), 'two\n');
    assert.equal(host.elevationProblem({installed, scripts, registered: true}), null);
    assert.match(host.elevationProblem({installed, scripts, registered: false}), /ButterpolloReleaseElevated is not registered.*elevation\.ps1/);
  } finally {
    fs.rmSync(dir, {recursive: true, force: true});
  }
});

test('the harness asks for the release scripts and its own, nothing under a user folder', () => {
  assert.deepEqual(Object.keys(host.ELEVATION.scripts).sort(), ['elevated-task.ps1', 'webrtc-e2e-elevated.ps1']);
  assert.equal(host.ELEVATION.installed, 'C:\\ProgramData\\ButterpolloRelease');
  assert.equal(host.ELEVATION.launcher, LAUNCHER);
  assert(fs.readFileSync(host.ELEVATION.scripts['webrtc-e2e-elevated.ps1'], 'utf8').includes(`$launcher = '${LAUNCHER}'`));
  for (const source of Object.values(host.ELEVATION.scripts)) assert(fs.existsSync(source), source);
  // elevated-task.ps1 and the installed script read the request from the folder the harness writes.
  const task = fs.readFileSync(host.ELEVATION.scripts['elevated-task.ps1'], 'utf8');
  const steps = fs.readFileSync(host.ELEVATION.scripts['webrtc-e2e-elevated.ps1'], 'utf8');
  assert(task.includes("$e2eDir = Join-Path $root 'webrtc-e2e'") && host.ELEVATION.requests === 'C:\\src\\butterpollo-release\\webrtc-e2e');
  assert(steps.includes(`$Dir = '${host.ELEVATION.requests}'`));
  assert(fs.readFileSync(path.join(__dirname, '..', '..', 'release', 'elevation.ps1'), 'utf8').includes("'webrtc-e2e-elevated.ps1'"));
});
