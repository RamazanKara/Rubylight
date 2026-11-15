import type { NeuralStatus } from './api';
import { megabytes, sentence } from './coop-view';

/**
 * What the game page's Neural Rendering panel shows for one app, from `GET /api/neural`. Pure, so
 * every state is checked from fixtures (NeuralView.test.mjs) without a host.
 *
 * `unavailable` (this PC's GPU can't run frame generation: the switch stays off and says why), `off`
 * and `on` (the frame generation switch; `kept` lists files a removal left because they changed
 * since), `working` (downloading, installing or removing, with progress), `choose` (the game's
 * program could not be found: the user picks it) and `failed` (the install, the removal or a
 * launch's check failed or was refused; the switch shows where it is and `title` says which of them
 * it was). `earlier` is the release of Rubylight 3.0's neural upscaling still in the game's folder,
 * which the panel offers only to remove, also after a failure that tells the user to remove it.
 */
export type NeuralView =
  | { kind: 'unavailable'; reason: string; earlier: string | null }
  | { kind: 'off'; kept: string[]; earlier: string | null }
  | { kind: 'on'; release: string | null; exe: string | null; addon: boolean }
  | { kind: 'working'; label: string; percent: number | null }
  | { kind: 'choose'; reason: string; earlier: string | null }
  | { kind: 'failed'; title: string; error: string; on: boolean; earlier: string | null };

/** The notice the user accepts once, before the first download. Cancel is the dialog's default. */
export const NOTICE = {
  title: 'Install a third-party game mod?',
  message:
    'Frame generation puts a third-party mod into the game’s folder, where the game loads it in place of its own DLSS frame generation. ' +
    'This PC downloads it from its authors’ GitHub release; it is not made or endorsed by AMD or NVIDIA.\n\n' +
    'Rubylight refuses games with a known anti-cheat in their install and games Steam lists Valve Anti-Cheat for, but an online game may still treat a mod as cheating. ' +
    'Use it at your own risk.',
  confirm: 'Accept and install',
};

/** What the panel says it is for. */
export const ABOUT =
  'Frame generation for games that offer DLSS frame generation, through a third-party mod Rubylight puts in the game’s folder and takes out again: turn on DLSS frame generation in the game, and this takes its place. ' +
  'Leave it off for online games: one may treat a mod as cheating.';

/** What turning the switch on does to the earlier upscaling. */
export const REPLACES = 'Turning on frame generation removes the earlier neural upscaling from the game’s folder first.';

export function neuralView(status: NeuralStatus): NeuralView {
  const job = status.job;
  const earlier = status.earlier_upscaling ? (status.earlier_upscaling.release ?? '') : null;
  if (job && job.phase !== 'done' && job.phase !== 'failed') {
    const percent = job.total > 0 ? Math.min(100, Math.round((job.done / job.total) * 100)) : null;
    const label =
      job.phase === 'download'
        ? job.total > 0
          ? `Downloading frame generation: ${megabytes(job.done)} of ${megabytes(job.total)}`
          : 'Checking the download…'
        : job.phase === 'place'
          ? 'Installing into the game’s folder…'
          : job.action === 'install'
            ? 'Removing the earlier neural upscaling…'
            : 'Removing from the game’s folder…';
    return { kind: 'working', label, percent: job.phase === 'remove' ? null : percent };
  }
  if (job?.phase === 'failed' && job.choose_exe) return { kind: 'choose', reason: sentence(job.error ?? ''), earlier };
  if (job?.phase === 'failed') {
    const title =
      job.action === 'remove'
        ? 'Neural Rendering couldn’t be removed'
        : job.action === 'update'
          ? 'Frame generation couldn’t be checked for the launch'
          : 'Frame generation couldn’t be installed';
    return { kind: 'failed', title, error: sentence(job.error ?? 'It failed.'), on: status.enabled, earlier };
  }
  if (status.enabled) return { kind: 'on', release: status.release, exe: status.exe, addon: status.mode === 'addon' };
  if (!status.available) return { kind: 'unavailable', reason: sentence(status.reason ?? 'This PC can’t run it.'), earlier };
  return { kind: 'off', kept: job?.action === 'remove' ? job.kept : [], earlier };
}

/** The panel polls while the host works. */
export function pollsNeural(view: NeuralView): boolean {
  return view.kind === 'working';
}
