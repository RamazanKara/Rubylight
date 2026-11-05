// Where /devices?pair=pin and /devices?pair=qr lead, the links behind the PC's pairing toast and
// its tray item. Null until that part of the page is there: a request can arrive after the page.
export type PairTarget = 'pin' | 'qr';

export function pairLinkTarget(want: string | null, waiting: number, qr: boolean): PairTarget | null {
  if (want === 'pin' && waiting > 0) return 'pin';
  if (want === 'qr' && qr) return 'qr';
  return null;
}
