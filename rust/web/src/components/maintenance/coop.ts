import type { CoopStatus } from '../../lib/api';
import type { Status } from '../StatusPill.svelte';

/** What the Nucleus Co-op install state means to the person at the console, as a StatusPill like the other Help panels. */
export const installStates: Record<CoopStatus['install']['state'], { label: string; status: Status }> = {
  installed: { label: 'Installed', status: 'ready' },
  configured: { label: 'Your own copy', status: 'ready' },
  missing: { label: 'Not set up yet', status: 'idle' },
  installing: { label: 'Downloading', status: 'busy' },
  failed: { label: 'Setup failed', status: 'error' },
};
