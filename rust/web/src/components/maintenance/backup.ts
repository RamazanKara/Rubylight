// Troubleshoot's settings backup: what the panel shows of a checked archive.
import type { BackupChange, BackupInspection } from '../../lib/api';

/** The largest archive the host takes (`settings_backup::MAX_ARCHIVE`). */
export const BACKUP_LIMIT = 512 * 1024 * 1024;

const LABELS: Record<BackupChange['item'], string> = {
  settings: 'Settings',
  apps: 'Apps',
  covers: 'Covers',
  game_settings: 'Game settings files',
  devices: 'Devices',
  pairing: 'Pairing identity',
  credentials: 'Sign-in and API tokens',
  webhooks: 'Webhooks',
};

export function changeLabel(item: BackupChange['item']): string {
  return LABELS[item] ?? item;
}

/** The archive's file name for a download, with the local date and time, as the host names it. */
export function backupFileName(secrets: boolean, now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `rubylight-settings-${stamp}${secrets ? '-with-secrets' : ''}.zip`;
}

/** Why a chosen file is refused before it is uploaded, or '' when it may go. */
export function fileProblem(file: { name: string; size: number }): string {
  if (file.size === 0) return 'The file is empty.';
  if (file.size > BACKUP_LIMIT) return 'The file is larger than a settings backup can be (512 MiB).';
  if (!file.name.toLowerCase().endsWith('.zip')) return 'Choose the .zip file a settings backup was saved as.';
  return '';
}

/** The sentence above the Restore button: what happens, and the secrets warning when they are in it. */
export function restoreNote(inspection: BackupInspection): string {
  const made = new Date(inspection.summary.created_unix * 1000);
  const when = Number.isNaN(made.getTime()) ? '' : ` from ${made.toLocaleString()}`;
  const base = `Backup${when}, made by Rubylight ${inspection.summary.host_version}. Restoring saves the current settings first, then restarts Rubylight; streams end.`;
  return inspection.summary.secrets
    ? `${base} It holds a pairing identity and sign-in: this PC takes them over, and devices paired since the backup must pair again.`
    : base;
}
