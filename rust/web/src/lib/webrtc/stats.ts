/**
 * What the play page's stats capsule shows, read from the browser's own `getStats()`.
 *
 * Nothing here reaches the host: the numbers are the browser's count of what it received and
 * decoded. Frame rate and bitrate come from two looks a moment apart, so the first look gives
 * neither. Pure, so Play.test.mjs tests it in Node.
 */

/** One entry of an `RTCStatsReport`, as far as the capsule reads it. */
export type Stat = {
  type?: string;
  kind?: string;
  id?: string;
  bytesReceived?: number;
  framesDecoded?: number;
  frameWidth?: number;
  frameHeight?: number;
  packetsLost?: number;
  codecId?: string;
  mimeType?: string;
  state?: string;
  nominated?: boolean;
  currentRoundTripTime?: number;
};

export type StatsReport = { forEach(visit: (stat: Stat) => void): void };

/** What one look counted, kept for the next look's rates. */
export type Look = { at: number; bytes: number; frames: number };

export type StreamStats = {
  /** Frames decoded per second since the last look; null on the first. */
  fps: number | null;
  /** Megabits per second received since the last look; null on the first. */
  mbps: number | null;
  /** Round trip to the PC in milliseconds; null when the browser does not say. */
  rttMs: number | null;
  /** The codec the browser decodes, as it names it: "H265", "AV1". Empty when unknown. */
  codec: string;
  width: number;
  height: number;
};

/** The video's numbers from `report`, with rates against `previous` (the look before, if any). */
export function readStats(report: StatsReport, previous: Look | null, now: number): { look: Look | null; stats: StreamStats } {
  let video: Stat | null = null;
  let rtt: number | null = null;
  const codecs = new Map<string, string>();
  report.forEach((stat) => {
    if (stat.type === 'inbound-rtp' && stat.kind === 'video') video = stat;
    else if (stat.type === 'codec' && stat.id && stat.mimeType) codecs.set(stat.id, stat.mimeType);
    else if (stat.type === 'candidate-pair' && (stat.nominated || stat.state === 'succeeded') && typeof stat.currentRoundTripTime === 'number') {
      rtt = stat.currentRoundTripTime * 1000;
    }
  });
  const inbound = video as Stat | null;
  const stats: StreamStats = { fps: null, mbps: null, rttMs: rtt, codec: '', width: 0, height: 0 };
  if (!inbound) return { look: null, stats };
  const look: Look = { at: now, bytes: inbound.bytesReceived ?? 0, frames: inbound.framesDecoded ?? 0 };
  stats.codec = (codecs.get(inbound.codecId ?? '') ?? '').replace(/^video\//i, '').toUpperCase();
  stats.width = inbound.frameWidth ?? 0;
  stats.height = inbound.frameHeight ?? 0;
  const seconds = previous ? (now - previous.at) / 1000 : 0;
  // A new stream counts from zero again: no rate across the restart.
  if (previous && seconds > 0 && look.bytes >= previous.bytes && look.frames >= previous.frames) {
    stats.fps = (look.frames - previous.frames) / seconds;
    stats.mbps = ((look.bytes - previous.bytes) * 8) / seconds / 1_000_000;
  }
  return { look, stats };
}

/** A capsule value in words: a dash until the browser has a number. */
export function statText(value: number | null, digits = 0): string {
  return value === null || !Number.isFinite(value) ? '–' : value.toFixed(digits);
}
