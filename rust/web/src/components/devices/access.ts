// What a paired device may do, in words: the presets the pairing choice and the device page
// offer. The host keeps the same masks (`butterpollo_core::state`): `play` is
// PLAY_DEVICE_PERMISSION, `view` LATER_DEVICE_PERMISSION and `all` FIRST_DEVICE_PERMISSION.
import { FULL_CONTROL, PERM, PERM_ALL, type PairingAccess } from '../../lib/api';

export type Access = PairingAccess;

const INPUT = PERM.CONTROLLER | PERM.TOUCH | PERM.PEN | PERM.MOUSE | PERM.KEYBOARD;
const FILES = PERM.FILE_UPLOAD | PERM.FILE_DOWNLOAD;

export const ACCESS_PERM: Record<Access, number> = {
  play: PERM.LIST_APPS | PERM.VIEW_STREAMS | PERM.LAUNCH_APPS | INPUT,
  view: PERM.LIST_APPS | PERM.VIEW_STREAMS,
  all: FULL_CONTROL,
};

export const ACCESS_CHOICES: { id: Access; label: string; hint: string }[] = [
  { id: 'play', label: 'Play games', hint: 'Start games, join co-op games, and use the controller, touch, mouse and keyboard.' },
  { id: 'view', label: 'Watch only', hint: 'See the stream, but not start games or use any controls.' },
  { id: 'all', label: 'Everything', hint: 'Also the clipboard and host commands. File transfer is granted on its own.' },
];

/** The preset `perm` matches, or `custom`. File transfer, granted on its own, keeps Play games and Everything. */
export function accessOf(perm: number): Access | 'custom' | 'none' {
  const granted = perm & PERM_ALL;
  if (granted === 0) return 'none';
  if (granted === ACCESS_PERM.view) return 'view';
  const rest = granted & ~FILES;
  if (rest === ACCESS_PERM.play) return 'play';
  if (rest === ACCESS_PERM.all) return 'all';
  return 'custom';
}

/** `perm` set to `access`: Watch only takes file transfer away, the others keep the file transfer bits `perm` has. */
export function withAccess(perm: number, access: Access): number {
  return access === 'view' ? ACCESS_PERM.view : ACCESS_PERM[access] | (perm & FILES);
}

/**
 * "Can join co-op games" (`coop_join`) once a preset is chosen: the preset decides (null follows Launch
 * apps), but a device turned off stays off while the preset still launches apps. Only a preset
 * without Launch apps, which can't join anyway, clears it.
 */
export function coopJoinAfterAccess(coopJoin: boolean | null, perm: number): boolean | null {
  return coopJoin === false && (perm & PERM.LAUNCH_APPS) !== 0 ? false : null;
}

/**
 * `coop_join` once the switch is flipped to `next`: landing back on the saved value keeps it as stored,
 * on follows Launch apps (null) so a later preset carries it along, and off is an explicit false.
 */
export function coopJoinAfterSwitch(next: boolean, saved: boolean | null): boolean | null {
  if (next === (saved ?? true)) return saved;
  return next ? null : false;
}

/** "Play games", "Watch only", "Everything", "No access" or "Custom". */
export function accessLabel(perm: number): string {
  const access = accessOf(perm);
  if (access === 'none') return 'No access';
  if (access === 'custom') return 'Custom';
  return ACCESS_CHOICES.find((choice) => choice.id === access)!.label;
}
