// Runs in a worker as an RTCRtpScriptTransform on the audio receiver. A browser plays remote audio
// through its output device's layout (stereo on most PCs), so Web Audio never sees 5.1 or 7.1. The
// encoded multistream packets are decoded here instead, with the channel mapping the client
// answered with, and each decoded channel is measured for its own tone with a Goertzel filter.
// The packets pass on untouched.
const WINDOW = 24000; // half a second at 48 kHz
let decoder = null;
let tones = [];
let channels = 0;
const filters = [];
let seen = 0;

function opusHead(o) {
  const head = new Uint8Array(21 + o.mapping.length);
  head.set(new TextEncoder().encode('OpusHead'));
  const view = new DataView(head.buffer);
  head[8] = 1; head[9] = o.channels;
  view.setUint32(12, 48000, true);
  head[18] = 1; head[19] = o.streams; head[20] = o.coupled;
  head.set(o.mapping, 21);
  return head;
}

function reset() {
  filters.length = 0;
  for (let c = 0; c < channels; c++) filters.push(tones.map(() => ({s1: 0, s2: 0})));
  seen = 0;
}

// WebCodecs decodes multistream Opus with FFmpeg, which treats the mapping as Vorbis order (FL FC
// FR ...) and returns the channels in FFmpeg's layout (FL FR FC LFE ...). A WebRTC receiver does
// not reorder: its output channel i is the encoder's input channel i. `NATIVE[i]` is where the
// receiver's channel i ended up in the WebCodecs output.
const NATIVE = {6: [0, 2, 1, 4, 5, 3], 8: [0, 2, 1, 6, 7, 4, 5, 3]};

function accept(data) {
  const plane = new Float32Array(data.numberOfFrames);
  for (let c = 0; c < Math.min(channels, data.numberOfChannels); c++) {
    data.copyTo(plane, {planeIndex: NATIVE[channels][c], format: 'f32-planar'});
    for (let t = 0; t < tones.length; t++) {
      const f = filters[c][t];
      const coeff = 2 * Math.cos(2 * Math.PI * tones[t] / 48000);
      let s1 = f.s1, s2 = f.s2;
      for (let i = 0; i < plane.length; i++) { const s0 = plane[i] + coeff * s1 - s2; s2 = s1; s1 = s0; }
      f.s1 = s1; f.s2 = s2;
    }
  }
  seen += data.numberOfFrames;
  if (seen >= WINDOW) {
    // Amplitude of each tone in dBFS: 2 * |X| / N.
    const levels = filters.map(per => per.map((f, t) => {
      const coeff = 2 * Math.cos(2 * Math.PI * tones[t] / 48000);
      const power = f.s1 * f.s1 + f.s2 * f.s2 - coeff * f.s1 * f.s2;
      return Math.round(20 * Math.log10(Math.max(2 * Math.sqrt(Math.max(power, 0)) / seen, 1e-9)));
    }));
    postMessage({type: 'levels', levels, decodedChannels: data.numberOfChannels});
    reset();
  }
  data.close();
}

async function setup(o) {
  channels = o.channels; tones = o.tones;
  reset();
  const config = {codec: 'opus', sampleRate: 48000, numberOfChannels: o.channels, description: opusHead(o)};
  const support = await AudioDecoder.isConfigSupported(config);
  postMessage({type: 'support', supported: support.supported, config: {codec: config.codec, numberOfChannels: config.numberOfChannels}});
  if (!support.supported) return;
  decoder = new AudioDecoder({output: accept, error: e => postMessage({type: 'error', message: String(e)})});
  decoder.configure(config);
}

let first = null;
function feed(frame) {
  if (!decoder || decoder.state !== 'configured') return;
  first ??= frame.timestamp;
  const micros = Math.round(((frame.timestamp - first) >>> 0) * 1e6 / 48000);
  decoder.decode(new EncodedAudioChunk({type: 'key', timestamp: micros, data: frame.data}));
}

onrtctransform = async event => {
  const {readable, writable} = event.transformer;
  const ready = setup(event.transformer.options).catch(e => postMessage({type: 'error', message: String(e)}));
  await ready;
  await readable.pipeThrough(new TransformStream({
    transform(frame, controller) { try { feed(frame); } catch (e) { postMessage({type: 'error', message: String(e)}); } controller.enqueue(frame); },
  })).pipeTo(writable);
};
