// What the live command stage shows about a stream, worked out from what the host reports.
// Nothing here invents a figure: a value the host did not send is left out, never guessed.
import { CODEC_NAMES, type StageTimings, type StreamSession } from '../../lib/api';

const ENCODERS: Record<string, string> = { amf: 'AMD AMF', nvenc: 'NVIDIA NVENC', pyrowave: 'PyroWave' };

/** A stream's codec in plain words. */
export function codecName(stream: StreamSession): string {
  return (CODEC_NAMES as readonly string[])[stream.video_format] ?? 'Unknown video format';
}

/** The short facts under the stage's title: picture, rate, range, codec, encoder. */
export function specPills(stream: StreamSession): string[] {
  const pills = [`${stream.width}×${stream.height}`, `${stream.fps} fps`];
  if (stream.hdr) pills.push('HDR');
  if (stream.vrr) pills.push('VRR');
  pills.push(codecName(stream));
  const encoder = ENCODERS[stream.encoder];
  if (encoder) pills.push(encoder);
  return pills;
}

export type PhaseId = 'capture' | 'convert' | 'encode' | 'send';
export interface Phase {
  id: PhaseId;
  label: string;
  /** Mean milliseconds a new picture spent here. */
  ms: number;
  /** Of the phases shown, 0..1. */
  share: number;
}

const PHASES: { id: PhaseId; label: string; parts: (keyof StageTimings)[] }[] = [
  { id: 'capture', label: 'Capture', parts: ['capture', 'hold', 'claim'] },
  { id: 'convert', label: 'Convert', parts: ['convert'] },
  { id: 'encode', label: 'Encode', parts: ['encode', 'queue'] },
  { id: 'send', label: 'Pace and send', parts: ['packetize', 'send_queue', 'send'] },
];

/**
 * The pipeline's four phases with the time a new picture spent in each. A phase the host timed at
 * nothing is left out; no stage timings, or a still picture (no new pictures in the window), gives none.
 */
export function pipelinePhases(stages: StageTimings | undefined, measured: boolean): Phase[] {
  if (!measured || !stages || !(stages.frames > 0)) return [];
  const timed = PHASES.map(({ id, label, parts }) => {
    let ms = 0;
    for (const part of parts) {
      const timing = stages[part];
      const mean = typeof timing === 'object' && timing ? timing.mean_ms : 0;
      if (Number.isFinite(mean) && mean > 0) ms += mean;
    }
    return { id, label, ms };
  }).filter((phase) => phase.ms > 0);
  const total = timed.reduce((sum, phase) => sum + phase.ms, 0);
  return timed.map((phase) => ({ ...phase, share: phase.ms / total }));
}

/** Milliseconds for a readout: two decimals under one, one under ten, whole above. */
export function shortMs(value: number): string {
  if (!Number.isFinite(value)) return '–';
  return value < 1 ? value.toFixed(2) : value < 10 ? value.toFixed(1) : String(Math.round(value));
}

export interface Trace {
  id: 'fps' | 'bitrate' | 'host';
  label: string;
  unit: string;
  /** The figure now, or a dash before a frame was measured. */
  now: string;
  /** One line under the figure. */
  sub: string;
  /** One value a second, the oldest first. */
  values: number[];
}

/** The stage's three traces: frame rate, bitrate and the host's time per frame. */
export function traces(stream: StreamSession): Trace[] {
  const perf = stream.performance;
  const measured = perf.sample_frames > 0;
  const history = perf.history ?? [];
  const mbps = perf.bitrate_mbps;
  return [
    {
      id: 'fps', label: 'Frame rate', unit: 'fps',
      now: measured ? String(Math.round(perf.fps)) : '–',
      sub: `Target ${stream.fps}`,
      values: history.map((sample) => sample.fps),
    },
    {
      id: 'bitrate', label: 'Bitrate', unit: 'Mbps',
      now: measured ? mbps.toFixed(mbps < 100 ? 1 : 0) : '–',
      sub: `Encoder set to ${(stream.encoder_bitrate_kbps / 1000).toFixed(stream.encoder_bitrate_kbps < 100_000 ? 1 : 0)}`,
      values: history.map((sample) => sample.bitrate_mbps),
    },
    {
      id: 'host', label: 'Host frame time', unit: 'ms',
      now: measured ? shortMs(perf.host_processing_mean_ms) : '–',
      sub: measured ? `99% under ${shortMs(perf.host_processing_p99_ms)} ms` : 'No new pictures to time',
      values: history.map((sample) => sample.host_processing_mean_ms),
    },
  ];
}

export type HealthLevel = 'healthy' | 'attention' | 'action';
export interface Health {
  level: HealthLevel;
  /** The word shown beside the colour. */
  label: 'Healthy' | 'Attention' | 'Action needed';
  /** One plain sentence: what is happening. */
  cause: string;
}

const LABELS: Record<HealthLevel, Health['label']> = { healthy: 'Healthy', attention: 'Attention', action: 'Action needed' };
const health = (level: HealthLevel, cause: string): Health => ({ level, label: LABELS[level], cause });
const STUTTER_WORDS: Record<string, string> = {
  capture: 'capturing the picture', convert: 'converting it', encode: 'encoding it', packetize: 'packing it', send: 'sending it',
};

/** How a stream is doing, in one word and one sentence, from the host's own findings. */
export function streamHealth(stream: StreamSession): Health {
  if (stream.state === 'STOPPING') return health('attention', 'This stream is stopping.');
  if (stream.pyrowave_minimum_kbps != null && stream.encoder_bitrate_kbps < stream.pyrowave_minimum_kbps) {
    return health('action', `The bitrate is too low for a clean picture. Raise it to ${Math.ceil((stream.pyrowave_recommended_kbps ?? stream.pyrowave_minimum_kbps) / 1000)} Mbps or more on the device.`);
  }
  const warning = stream.warnings?.[0];
  if (warning) return health('attention', warning.message);
  const link = stream.link_rate;
  if (link && (link.state === 'limited' || link.state === 'floor')) {
    const to = (stream.encoder_bitrate_kbps / 1000).toFixed(stream.encoder_bitrate_kbps < 10_000 ? 1 : 0);
    return health('attention', link.reason === 'send_losses'
      ? `This PC could not send fast enough, so the bitrate is down to ${to} Mbps.`
      : `The network lost packets, so the bitrate is down to ${to} Mbps.`);
  }
  if (stream.pyrowave_recommended_kbps != null && stream.encoder_bitrate_kbps < stream.pyrowave_recommended_kbps) {
    return health('attention', 'The bitrate is below the recommended level: text and textures may lose detail.');
  }
  const stutter = stream.stutter;
  if (stutter && stutter.pictures >= 100 && stutter.late_pictures / stutter.pictures > 0.05) {
    const where = stutter.dominant ? STUTTER_WORDS[stutter.dominant] : '';
    return health('attention', `Some pictures leave this PC late${where ? `, most of the time lost ${where}` : ''}.`);
  }
  if (stream.performance.sample_frames <= 0) return health('healthy', 'The picture is still: nothing new to send.');
  return health('healthy', 'Every stage on this PC is keeping up.');
}

/** The worst of several streams' health, for the stage's one banner. */
export function worstHealth(streams: StreamSession[]): Health | null {
  const order: HealthLevel[] = ['action', 'attention', 'healthy'];
  const all = streams.map(streamHealth);
  for (const level of order) {
    const found = all.find((item) => item.level === level);
    if (found) return found;
  }
  return null;
}

/** The stream the stage leads with: the first that runs, else the first that is stopping. */
export function leadStream(streams: StreamSession[]): StreamSession | undefined {
  return streams.find((stream) => stream.state === 'RUNNING') ?? streams[0];
}

/** The stage's title: the game on the device, or just the device. */
export function stageTitle(streams: StreamSession[], game: string | null): string {
  const device = leadStream(streams)?.device_name || 'a device';
  const others = streams.length - 1;
  const more = others > 0 ? ` and ${others} more` : '';
  return game ? `${game} on ${device}${more}` : `Streaming to ${device}${more}`;
}

/** A stream's figures as plain text, for a support request. No address, no device id. */
export function diagnosticsText(stream: StreamSession, game: string | null): string {
  const perf = stream.performance;
  const measured = perf.sample_frames > 0;
  const lines = [
    `Rubylight stream${game ? `: ${game}` : ''}`,
    `Picture: ${specPills(stream).join(', ')}`,
    `Bitrate: ${(stream.encoder_bitrate_kbps / 1000).toFixed(1)} Mbps set${measured ? `, ${perf.bitrate_mbps.toFixed(1)} Mbps sent` : ''}`,
  ];
  if (measured) {
    lines.push(`Frame rate: ${Math.round(perf.fps)} of ${stream.fps} fps`);
    lines.push(`Host frame time: ${shortMs(perf.host_processing_mean_ms)} ms average, 99% under ${shortMs(perf.host_processing_p99_ms)} ms`);
    lines.push(`Encoding: ${shortMs(perf.encode_mean_ms)} ms average, 95% under ${shortMs(perf.encode_p95_ms)} ms`);
  } else {
    lines.push('Frame rate: no new pictures in the last seconds (a still picture)');
  }
  const phases = pipelinePhases(perf.stages, measured);
  if (phases.length) lines.push(`Pipeline: ${phases.map((phase) => `${phase.label} ${shortMs(phase.ms)} ms`).join(', ')}`);
  const link = stream.link_rate;
  if (link && link.state !== 'off') lines.push(`Network bitrate control: ${link.state}, ceiling ${(link.ceiling_kbps / 1000).toFixed(1)} Mbps`);
  for (const warning of stream.warnings ?? []) lines.push(`Warning: ${warning.message}`);
  const state = streamHealth(stream);
  lines.push(`Health: ${state.label}. ${state.cause}`);
  return lines.join('\n');
}
