// The Stream page's guests (docs/browser-guests.md): what an invite may do, how long it lasts and
// the link it makes. Pure, so it is tested in Node (Guests.test.mjs).
import type { GuestPermission } from './api';

export const PERMISSIONS: readonly { value: GuestPermission; label: string }[] = [
  { value: 'view', label: 'View only' },
  { value: 'controller', label: 'Controller' },
  { value: 'controller-keyboard-mouse', label: 'Controller, keyboard and mouse' },
];

export const permissionLabel = (permission: GuestPermission) => PERMISSIONS.find((p) => p.value === permission)?.label ?? permission;

/** The host keeps a link five minutes to seven days (`butterpollo_core::webrtc::guests`). */
export const MIN_SECONDS = 5 * 60;
export const MAX_SECONDS = 7 * 86_400;

export type Expiry = '1h' | '24h' | 'custom';
export const EXPIRIES: readonly { value: Expiry; label: string }[] = [
  { value: '1h', label: '1 hour' },
  { value: '24h', label: '24 hours' },
  { value: 'custom', label: 'Custom' },
];
export const UNITS: readonly { value: string; label: string }[] = [
  { value: '60', label: 'Minutes' },
  { value: '3600', label: 'Hours' },
  { value: '86400', label: 'Days' },
];

/** How long a link lasts, in seconds; null when the custom amount is outside what the host keeps. */
export function lifetime(expiry: Expiry, amount: number, unit: number): number | null {
  if (expiry === '1h') return 3600;
  if (expiry === '24h') return 86_400;
  const seconds = Math.round(amount * unit);
  return Number.isFinite(seconds) && seconds >= MIN_SECONDS && seconds <= MAX_SECONDS ? seconds : null;
}

const loopback = (hostname: string) => {
  const name = hostname.replace(/^\[|\]$/g, '').toLowerCase();
  return name === 'localhost' || name === '::1' || name.endsWith('.localhost') || /^127\./.test(name);
};

/**
 * The address a guest opens the link at: the console's host, or for a console opened on this PC
 * (`localhost`), the PC's first LAN address; on the guest page's own port.
 */
export function linkAddress(page: { hostname: string }, addresses: readonly string[], port: number): string {
  const host = loopback(page.hostname) && addresses[0] ? addresses[0] : page.hostname;
  return `${host}:${port}`;
}

/** `address` with the guest port when it names none: a host name alone would open port 443. */
export function withPort(address: string, port: number): string {
  const trimmed = address.trim();
  return /:\d{1,5}$/.test(trimmed) && !/^[^[]*:[^\]]*:/.test(trimmed) ? trimmed : `${trimmed}:${port}`;
}

/** An address the link can carry: a host name or IP with an optional port, nothing else. */
export function validAddress(address: string): boolean {
  return /^(\[[0-9a-f:.]+\]|[a-z0-9.-]+)(:\d{1,5})?$/i.test(address.trim());
}

/** Why a guest couldn't open a link at `address`, or undefined when one can. */
export function addressProblem(address: string): string | undefined {
  if (!validAddress(address)) return 'A host name or IP address, with an optional port';
  if (loopback(address.trim().replace(/:\d{1,5}$/, ''))) return 'Only this PC opens this address';
  return undefined;
}

/** The token goes in the fragment: browsers never send it, so no log or proxy sees it. */
export function guestLink(address: string, token: string): string {
  return `https://${address.trim()}/guest/#${token}`;
}

/** "Expires in 59 min", "in 23 h", "in 6 days" from Unix seconds; "Expired" once it has. */
export function expiresIn(expires: number, now = Date.now() / 1000): string {
  if (expires <= now) return 'expired';
  const left = expires - now;
  if (left < 3600) return `in ${Math.max(1, Math.ceil(left / 60))} min`;
  if (left < 2 * 86_400) return `in ${Math.round(left / 3600)} h`;
  return `in ${Math.round(left / 86_400)} days`;
}
