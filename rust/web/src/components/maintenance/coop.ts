import type { CoopInstall } from '../../lib/api';
import type { Status } from '../StatusPill.svelte';

/** What the Nucleus Co-op install state means to the person at the console, as a StatusPill like the other Help panels. */
export const installStates: Record<CoopInstall['state'], { label: string; status: Status }> = {
  installed: { label: 'Earlier install', status: 'ready' },
  configured: { label: 'Found', status: 'ready' },
  missing: { label: 'Not set up yet', status: 'idle' },
  failed: { label: 'Can’t run', status: 'error' },
};

/** Why the folder chosen can't run it, in the pill; the banner below gives the host's reason. */
const problems: Record<NonNullable<CoopInstall['problem']>, string> = {
  missing: 'Not found',
  version: 'Wrong version',
  untrusted: 'Folder refused',
};

export function installPill(install: CoopInstall): { label: string; status: Status } {
  const pill = installStates[install.state];
  return install.state === 'failed' && install.problem ? { ...pill, label: problems[install.problem] } : pill;
}
