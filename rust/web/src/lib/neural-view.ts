import type { NeuralStatus } from './api';
import { megabytes, sentence } from './coop-view';

/**
 * What the game page's Neural Rendering panel shows for one app, from `GET /api/neural`. Pure, so
 * every state is checked from fixtures (NeuralView.test.mjs) without a host.
 *
 * `unavailable` (this PC's GPU or driver can't run it: the switch stays off and says why), `off`
 * and `on` (the switch; `kept` lists files a removal left because they changed since), `working`
 * (downloading, installing or removing, with progress), `choose` (the game's program could not be
 * found: the user picks it) and `failed` (the install or removal failed or was refused; the switch
 * shows where it is).
 */
export type NeuralView =
  | { kind: 'unavailable'; reason: string }
  | { kind: 'off'; kept: string[] }
  | { kind: 'on'; release: string | null; exe: string | null }
  | { kind: 'working'; label: string; percent: number | null }
  | { kind: 'choose'; reason: string }
  | { kind: 'failed'; error: string; on: boolean };

/** The notice the user accepts once, before the first download. Cancel is the dialog's default. */
export const NOTICE = {
  title: 'Install a third-party game mod?',
  message:
    'Neural Rendering puts a third-party mod into the game’s folder, where the game loads it. ' +
    'This PC downloads it from its authors’ GitHub release; it is not made or endorsed by AMD or NVIDIA.\n\n' +
    'Rubylight refuses games with anti-cheat, but an online game may still treat a mod as cheating. ' +
    'Use it at your own risk.',
  confirm: 'Accept and install',
};

/** What the panel says it is for. */
export const ABOUT =
  'Neural upscaling for games with DLSS, FSR or XeSS, through a third-party mod Rubylight puts in the game’s folder and takes out again. It needs about 2 GB of video memory.';

export function neuralView(status: NeuralStatus): NeuralView {
  const job = status.job;
  if (job && job.phase !== 'done' && job.phase !== 'failed') {
    const percent = job.total > 0 ? Math.min(100, Math.round((job.done / job.total) * 100)) : null;
    const label =
      job.phase === 'download'
        ? job.total > 0
          ? `Downloading Neural Rendering: ${megabytes(job.done)} of ${megabytes(job.total)}`
          : 'Looking for the newest release…'
        : job.phase === 'place'
          ? 'Installing into the game’s folder…'
          : 'Removing from the game’s folder…';
    return { kind: 'working', label, percent: job.phase === 'remove' ? null : percent };
  }
  if (job?.phase === 'failed' && job.choose_exe) return { kind: 'choose', reason: sentence(job.error ?? '') };
  if (job?.phase === 'failed') return { kind: 'failed', error: sentence(job.error ?? 'It failed.'), on: status.enabled };
  if (status.enabled) return { kind: 'on', release: status.release, exe: status.exe };
  if (!status.available) return { kind: 'unavailable', reason: sentence(status.reason ?? 'This PC can’t run it.') };
  return { kind: 'off', kept: job?.action === 'remove' ? job.kept : [] };
}

/** The panel polls while the host works. */
export function pollsNeural(view: NeuralView): boolean {
  return view.kind === 'working';
}
