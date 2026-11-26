// The pairing flow's three steps and the one sentence that says what to do now. The step follows
// what the host reports: a PIN request that waits, a device that just paired. Nothing is timed.

export type PairingStage = 'code' | 'confirm' | 'ready';

export interface PairingFacts {
  /** The device that paired while the panel was open. */
  welcome?: string | null;
  /** Names of the devices whose PIN request waits. */
  pending?: string[];
  /** Pairing is turned off on this PC. */
  off?: boolean;
  /** A QR code shows (Nightjar scans it). */
  qr?: boolean;
}

/** Where the flow stands. */
export function pairingStage({ welcome, pending = [], off = false }: PairingFacts): PairingStage {
  if (welcome) return 'ready';
  return !off && pending.length > 0 ? 'confirm' : 'code';
}

export interface PairingStep {
  id: PairingStage;
  title: string;
  state: 'done' | 'now' | 'next';
}

const TITLES: [PairingStage, string][] = [['code', 'Show the code'], ['confirm', 'Confirm the device'], ['ready', 'Ready to play']];

/**
 * The rail: every step before the current one is done. Once a device is ready, all are. While
 * pairing is off no code shows, so no step is the current one.
 */
export function pairingSteps(stage: PairingStage, off = false): PairingStep[] {
  const at = off && stage === 'code' ? -1 : TITLES.findIndex(([id]) => id === stage);
  return TITLES.map(([id, title], index) => ({
    id,
    title,
    state: index < at || stage === 'ready' ? 'done' : index === at ? 'now' : 'next',
  }));
}

/** One plain sentence for the current step. */
export function pairingLead(facts: PairingFacts): string {
  const stage = pairingStage(facts);
  if (stage === 'ready') return `${facts.welcome} is paired and can play from this PC now.`;
  if (stage === 'confirm') {
    const pending = facts.pending ?? [];
    return pending.length === 1
      ? `${pending[0]} asks to pair. Type the PIN it shows, if it is the device you expect.`
      : `${pending.length} devices ask to pair. Type each PIN, if it is a device you expect.`;
  }
  if (facts.off) return 'Pairing is off, so no new device can join. Devices already paired keep working.';
  return facts.qr
    ? 'Scan the code with Nightjar. Moonlight and other apps show a PIN instead, which you type here.'
    : 'Add this PC in Moonlight or another app. It shows a PIN, which you type here.';
}
