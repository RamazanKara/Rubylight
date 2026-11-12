// The pairing moment: a device in the paired list that was not there when the pairing
// panel first saw it. The panel's QR tile then becomes that device's card.
import type { Client } from '../../lib/api';

/** The devices already paired: the first list the panel sees, so no device counts as new on load. */
export function knownDevices(list: Client[]): Set<string> {
  return new Set(list.map((client) => client.uuid));
}

/** The device paired since `known`, the latest in the list when several were; null when none was. */
export function newlyPaired(known: Set<string> | null, list: Client[] | null): Client | null {
  if (!known || !list) return null;
  return list.filter((client) => !known.has(client.uuid)).at(-1) ?? null;
}
