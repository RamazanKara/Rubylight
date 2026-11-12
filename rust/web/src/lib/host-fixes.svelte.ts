// The one-click fixes of Home's checks (host-readiness.ts): the hero and the checks list
// share what runs, so a fix started in one shows busy in the other.
import { api, ApiError, type PowerStatus } from './api';
import { failed, notify } from './feedback.svelte';
import { marked } from './format';
import { warningKey, type Check } from './host-readiness';
import { checkPowerStatus } from './power';
import { refreshMetadata } from './session.svelte';

export const fixes = $state({
  /** The id of the check whose fix or dismissal runs. */
  running: '',
  /** `warningKey`s dismissed here: hidden at once; the host keeps them hidden until their text changes. */
  dismissed: [] as string[],
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Applies the adapter changes, then waits for the host's result as the Power panel does:
 * it applies them about a second after it answers, and the adapters' restart may drop
 * this page's connection for a moment. The new wake status, or null when there's none.
 */
async function wake(changes: string[], previous: number | null): Promise<PowerStatus | null> {
  let result;
  try {
    result = await api.power.fixWake(changes, false);
  } catch (error) {
    failed('Wake settings not changed', error);
    // The adapters changed since the check: show what a fix would do now.
    const refresh = error instanceof ApiError && error.status === 409;
    return checkPowerStatus(await api.power.status(refresh));
  }
  if (!result.changes.length) {
    notify('Nothing to change: the wake settings are already on.', 'ok');
    return checkPowerStatus(await api.power.status());
  }
  let power: PowerStatus | null = null;
  const started = Date.now();
  while (Date.now() - started < 60_000) {
    await wait(2000);
    try {
      power = checkPowerStatus(await marked(() => api.power.status()));
    } catch {
      continue;
    }
    if (!power.fix.running) break;
  }
  const done = power?.fix.last;
  if (!power || power.fix.running) notify('The wake settings are still changing. Check again in a minute.', 'info');
  else if (!done || done.at === previous) notify('Rubylight restarted before it reported the wake settings. Open Wake settings to see them now.', 'danger');
  else if (done.errors.length) notify(`Some wake settings could not be changed: ${done.errors.join(' ')}`, 'danger');
  else notify('Devices can wake this PC now.', 'ok');
  return power;
}

/**
 * Runs a check's fix on a click; never on its own. `after` gets the new wake status when
 * the fix changed it, so the page shows it without polling.
 */
export async function applyFix(check: Check, after?: (power: PowerStatus | null) => void): Promise<void> {
  const fix = check.fix;
  if (!fix || fixes.running) return;
  fixes.running = check.id;
  try {
    if (fix.kind === 'wake') {
      const power = await wake(fix.changes, fix.previous).catch(() => null);
      after?.(power);
      return;
    }
    await api.config.patch(fix.values);
    await refreshMetadata().catch(() => {});
    notify(fix.done, 'ok');
    after?.(null);
  } catch (error) {
    failed('Changing the setting failed', error);
  } finally {
    fixes.running = '';
  }
}

/** Hides a host warning until its text changes. */
export async function dismissWarning(check: Check, after?: () => void): Promise<void> {
  if (!check.warning || fixes.running) return;
  fixes.running = check.id;
  try {
    await api.dismissWarning(check.warning);
    fixes.dismissed = [...fixes.dismissed, warningKey({ code: check.warning, message: check.text })];
    after?.();
  } catch (error) {
    failed('Dismissing the warning failed', error);
  } finally {
    fixes.running = '';
  }
}
