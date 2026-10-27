import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

function releaseBuild(environment = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'nightjar-release-'));
  try {
    mkdirSync(join(directory, 'scripts'));
    copyFileSync(new URL('./build-release.sh', import.meta.url), join(directory, 'scripts/build-release.sh'));
    writeFileSync(join(directory, 'gradlew'), `#!/usr/bin/env bash
set -eu
printf '%s\\n' "$*" >> invocations
if [[ "$*" == *lintNonRootRelease* ]]; then
    exit "$VALIDATION_EXIT"
fi
mkdir -p app/build/outputs/apk/nonRoot/release app/build/outputs/apk/root/release app/build/outputs/bundle/nonRootRelease
printf apk > app/build/outputs/apk/nonRoot/release/app-nonRoot-release.apk
printf root-apk > app/build/outputs/apk/root/release/app-root-release.apk
printf bundle > app/build/outputs/bundle/nonRootRelease/app-nonRoot-release.aab
`);
    const result = spawnSync('bash', [join(directory, 'scripts/build-release.sh')], {
      cwd: tmpdir(), encoding: 'utf8', env: { ...process.env, VALIDATION_EXIT: '0', ...environment },
    });
    return { ...result, invocations: readFileSync(join(directory, 'invocations'), 'utf8').trim().split('\n') };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('release helper validates before building a universal APK and bundle', () => {
  const result = releaseBuild();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.invocations.length, 2);
  assert.match(result.invocations[0], /:app:lintNonRootRelease/);
  assert.match(result.invocations[0], /:app:lintRootRelease/);
  assert.match(result.invocations[0], /:app:testNonRootDebugUnitTest/);
  assert.match(result.invocations[0], /:app:testRootDebugUnitTest/);
  assert.match(result.invocations[1], /:app:assembleNonRootRelease :app:assembleRootRelease :app:bundleNonRootRelease/);
  assert.doesNotMatch(result.invocations.join('\n'), /abiFilters/);
  assert.equal(result.stdout.match(/^[a-f0-9]{64}  app\//gm).length, 3);
});

test('release helper stops before packaging or printing hashes when validation fails', () => {
  const result = releaseBuild({ VALIDATION_EXIT: '7' });
  assert.equal(result.status, 7);
  assert.equal(result.invocations.length, 1);
  assert.equal(result.stdout, '');
});

test('PowerShell helper retains the same universal build and validation gates', () => {
  const script = readFileSync(new URL('./build-release.ps1', import.meta.url), 'utf8');
  assert.doesNotMatch(script, /abiFilters/);
  assert.match(script, /:app:lintNonRootRelease :app:lintRootRelease :app:testNonRootDebugUnitTest :app:testRootDebugUnitTest/);
  assert.match(script, /:app:assembleNonRootRelease :app:assembleRootRelease :app:bundleNonRootRelease/);
  assert.ok(script.indexOf("throw 'Release validation failed'") < script.indexOf(':app:assembleNonRootRelease'));
});
