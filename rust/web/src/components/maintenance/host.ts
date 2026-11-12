// Restarting Rubylight, from Troubleshoot's Rubylight panel and from the quick switcher (Ctrl+K).
import { confirm } from '../../lib/feedback.svelte';
import { refreshMetadata } from '../../lib/session.svelte';
import { sleep } from './util';

/** Asks before a restart: streams in progress end. */
export function confirmRestart(): Promise<boolean> {
  return confirm({
    title: 'Restart Rubylight?',
    message: 'Streams in progress disconnect. This console reconnects by itself when Rubylight is back.',
    confirm: 'Restart',
  });
}

/**
 * After the restart request: true once Rubylight has been away and answers again, false when it hasn't
 * come back within 90 seconds or `alive` says the caller is gone. Rubylight answers for a moment before
 * it stops, so an answer counts only after it was away, or after ten seconds.
 */
export async function backAfterRestart(alive: () => boolean = () => true): Promise<boolean> {
  const started = Date.now();
  let away = false;
  while (alive() && Date.now() - started < 90_000) {
    await sleep(1000);
    try {
      await refreshMetadata();
      if (away || Date.now() - started > 10_000) return true;
    } catch {
      away = true;
    }
  }
  return false;
}
