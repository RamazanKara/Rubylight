import type { CoopStatus } from '../../lib/api';

type Tone = 'neutral' | 'ok' | 'warn' | 'danger';

/** What the Nucleus Co-op install state means to the person at the console. */
export const installStates: Record<CoopStatus['install']['state'], { label: string; tone: Tone }> = {
  installed: { label: 'Installed', tone: 'ok' },
  configured: { label: 'Your own copy', tone: 'ok' },
  missing: { label: 'Not set up yet', tone: 'neutral' },
  installing: { label: 'Downloading', tone: 'neutral' },
  failed: { label: 'Setup failed', tone: 'danger' },
};
