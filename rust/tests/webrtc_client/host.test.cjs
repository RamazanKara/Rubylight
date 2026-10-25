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

// Runs the elevated script's checks on `steps` without running anything, and returns its answer.
function dryRun(steps) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'rl-elevate-'));
  try {
    fs.writeFileSync(path.join(dir, 'request.json'), JSON.stringify({id: 'test', steps}));
    execFileSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(__dirname, 'elevated_tasks.ps1'), '-Dir', dir, '-DryRun'], {stdio: 'pipe'});
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
  assert(task.includes("'webrtc-e2e\\request.json'") && host.ELEVATION.requests === 'C:\\src\\butterpollo-release\\webrtc-e2e');
  assert(steps.includes(`$Dir = '${host.ELEVATION.requests}'`));
  assert(fs.readFileSync(path.join(__dirname, '..', '..', 'release', 'elevation.ps1'), 'utf8').includes("'webrtc-e2e-elevated.ps1'"));
});
