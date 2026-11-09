import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

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
  assert.equal(result.stdout.match(/^[a-f0-9]{64} [ *]app\//gm)?.length, 3, result.stdout);
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

test('installer settings cleanup waits for deletion and still rejects wrong files', (t) => {
  const shell = process.platform === 'win32' ? 'powershell.exe' : 'pwsh';
  if (spawnSync(shell, ['-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()']).error?.code === 'ENOENT') {
    t.skip('PowerShell is unavailable');
    return;
  }
  const directory = mkdtempSync(join(tmpdir(), 'nightjar-settings-smoke-'));
  try {
    const source = fileURLToPath(new URL('../protocol/desktop/packaging/windows/build-installer.ps1', import.meta.url));
    assert.match(readFileSync(source, 'utf8'), /Wait-SettingsFiles \$config \$userFiles/);
    const script = join(directory, 'test.ps1');
    writeFileSync(script, String.raw`
$ErrorActionPreference = 'Stop'
$ast = [Management.Automation.Language.Parser]::ParseFile($env:NIGHTJAR_SMOKE_SOURCE, [ref]$null, [ref]$null)
$function = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Wait-SettingsFiles' }, $true)
if (-not $function) { throw 'Wait-SettingsFiles is missing' }
. ([scriptblock]::Create($function.Extent.Text))
$path = Join-Path $env:NIGHTJAR_SMOKE_TEST_DIR 'config'
New-Item -ItemType Directory -Force $path | Out-Null
$keep = Join-Path $path 'user.csv'
$remove = Join-Path $path 'settings.json'
Set-Content -LiteralPath $keep -Value 'keep'
Set-Content -LiteralPath $remove -Value 'remove'
$job = Start-Job -ArgumentList $remove -ScriptBlock {
    param($file)
    Start-Sleep -Milliseconds 500
    Remove-Item -LiteralPath $file
}
try {
    Wait-SettingsFiles $path @('user.csv') -TimeoutSeconds 15
    $job | Wait-Job | Receive-Job -ErrorAction Stop
} finally {
    $job | Remove-Job -Force
}
# A directory removed mid-enumeration must be retried, rather than ending the smoke test.
$script:enumerations = 0
function Get-ChildItem {
    [CmdletBinding()] param([string]$LiteralPath, [switch]$Recurse, [switch]$File)
    $script:enumerations++
    if ($script:enumerations -eq 1) {
        $errorRecord = [Management.Automation.ErrorRecord]::new([Management.Automation.ItemNotFoundException]::new('directory disappeared'), 'gone', [Management.Automation.ErrorCategory]::ObjectNotFound, $LiteralPath)
        $PSCmdlet.ThrowTerminatingError($errorRecord)
    }
    if ($script:enumerations -eq 2) {
        $errorRecord = [Management.Automation.ErrorRecord]::new([IO.DirectoryNotFoundException]::new('directory disappeared'), 'gone', [Management.Automation.ErrorCategory]::ReadError, $LiteralPath)
        $PSCmdlet.ThrowTerminatingError($errorRecord)
    }
    Microsoft.PowerShell.Management\Get-ChildItem @PSBoundParameters
}
Wait-SettingsFiles $path @('user.csv') -TimeoutSeconds 2
if ($script:enumerations -lt 3) { throw 'the transient enumeration failures were not retried' }
# Neither a leftover client file nor a lost protected file may pass.
Set-Content -LiteralPath $remove -Value 'leftover'
$failure = $null
try { Wait-SettingsFiles $path @('user.csv') -TimeoutSeconds 0 } catch { $failure = $_ }
if (-not $failure -or $failure.ToString() -notmatch 'settings.json') { throw 'leftover settings passed' }
Remove-Item -LiteralPath $remove, $keep
$failure = $null
try { Wait-SettingsFiles $path @('user.csv') -TimeoutSeconds 0 } catch { $failure = $_ }
if (-not $failure) { throw 'lost user files passed' }
`);
    const result = spawnSync(shell, ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script], {
      encoding: 'utf8', timeout: 30000,
      env: { ...process.env, NIGHTJAR_SMOKE_SOURCE: source, NIGHTJAR_SMOKE_TEST_DIR: directory },
    });
    assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
