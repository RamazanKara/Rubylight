// What the console's shell shows beside its pages: the live badge in the command bar.

/** What plays now, as the shell's poll last heard it. */
export interface LiveState {
  /** Streams running now. */
  streams: number;
  /** The running app's name, or nothing when none runs. */
  app: string;
}

export interface LiveBadge {
  label: string;
  title: string;
  /** A device is streaming: the badge is green and its dot beats. */
  streaming: boolean;
}

/**
 * The command bar's badge: nothing while the PC idles, the running game while one runs,
 * and "Live" with the game or the stream count while a device streams.
 */
export function liveBadge(live: LiveState | null): LiveBadge | null {
  if (!live) return null;
  if (live.streams > 0) {
    const what = live.app || (live.streams === 1 ? '1 stream' : `${live.streams} streams`);
    const count = live.streams === 1 ? 'One device is streaming' : `${live.streams} streams are running`;
    return { label: `Live · ${what}`, title: `${count}. Open Home for the live stage.`, streaming: true };
  }
  if (live.app) return { label: `${live.app} is running`, title: `${live.app} is running on this PC.`, streaming: false };
  return null;
}
