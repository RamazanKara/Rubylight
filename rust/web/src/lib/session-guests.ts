// Guests from paired devices on the Home page (docs/multiclient.md): who joined the running game,
// who waits to, and what each may do. Pure, so it is tested in Node (SessionGuests.test.mjs).
import type { GuestPermission, SessionGuestsStatus } from './api';
import { PERMISSIONS, permissionLabel } from './guests';

export { PERMISSIONS, permissionLabel };

/** How long a let-in guest may stay: until the game ends, or a time the host keeps (300 s to 7 days). */
export const STAYS: readonly { value: string; label: string; seconds: number | null }[] = [
  { value: 'game', label: 'Until the game ends', seconds: null },
  { value: '1h', label: '1 hour', seconds: 3600 },
  { value: '24h', label: '24 hours', seconds: 86_400 },
];

export const staySeconds = (value: string): number | null => STAYS.find((stay) => stay.value === value)?.seconds ?? null;

/** Whether the card has anything to show: guests, joins waiting, or a game others may join. */
export function shown(status: SessionGuestsStatus | null): boolean {
  if (!status) return false;
  return status.guests.length > 0 || status.waiting.length > 0;
}

/** "Phone · Controller", with the keyboard and mouse when this guest holds them. */
export function guestLine(status: SessionGuestsStatus, id: number): string {
  const guest = status.guests.find((g) => g.id === id);
  if (!guest) return '';
  const holds = status.keyboard_mouse === id ? ' · has the keyboard and mouse' : '';
  return `${permissionLabel(guest.permission)}${holds}`;
}

/** "Ends in 59 min" from Unix seconds; empty for a guest who stays until the game ends. */
export function endsIn(expires: number | null, now = Date.now() / 1000): string {
  if (expires == null) return '';
  const left = expires - now;
  if (left <= 0) return 'Ending';
  if (left < 3600) return `Ends in ${Math.max(1, Math.ceil(left / 60))} min`;
  if (left < 2 * 86_400) return `Ends in ${Math.round(left / 3600)} h`;
  return `Ends in ${Math.round(left / 86_400)} days`;
}

/** The permission a waiting device is let in with by default: the host's own default. */
export const initialPermission = (status: SessionGuestsStatus): GuestPermission => status.default_permission;

/** Whether `permission` lets a guest take the keyboard and mouse. */
export const usesKeyboardMouse = (permission: GuestPermission) => permission === 'controller-keyboard-mouse';
