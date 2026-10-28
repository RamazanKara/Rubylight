// In-page measurement for the WebRTC end-to-end legs: peer connection stats,
// the glass-to-glass time code read from decoded frames, and a per-channel
// tone check. Injected before the page's own scripts (page.addInitScript) so
// the browser client's module-private RTCPeerConnection can be reached too.
(function (root) {
  const peers = [];
  const Native = root.RTCPeerConnection;
  if (Native && !Native.__rl) {
    class Wrapped extends Native { constructor(...args) { super(...args); peers.push(this); } }
    Wrapped.__rl = true; root.RTCPeerConnection = Wrapped;
  }
  const probe = {peers, series: [], g2g: [], g2gRejected: 0, calibration: null};

  // ---- peer connection statistics ---------------------------------------
  probe.sample = async function () {
    const pc = peers[peers.length - 1];
    if (!pc) return null;
    const report = await pc.getStats();
    const by = {}; report.forEach(s => { by[s.id] = s; });
    const out = {at: Date.now()};
    report.forEach(s => {
      if (s.type !== 'inbound-rtp') return;
      const codec = by[s.codecId];
      const keep = ['packetsReceived', 'packetsLost', 'bytesReceived', 'jitter', 'framesReceived', 'framesDecoded', 'framesDropped', 'keyFramesDecoded',
        'freezeCount', 'totalFreezesDuration', 'pauseCount', 'jitterBufferDelay', 'jitterBufferEmittedCount', 'totalDecodeTime', 'totalInterFrameDelay',
        'nackCount', 'pliCount', 'firCount', 'frameWidth', 'frameHeight', 'concealedSamples', 'totalSamplesReceived', 'concealmentEvents', 'insertedSamplesForDeceleration',
        'removedSamplesForAcceleration', 'jitterBufferTargetDelay', 'decoderImplementation'];
      const entry = {codec: codec ? codec.mimeType : null, fmtp: codec ? codec.sdpFmtpLine : null};
      for (const key of keep) if (s[key] !== undefined) entry[key] = s[key];
      out[s.kind] = entry;
    });
    report.forEach(s => {
      if (s.type === 'candidate-pair' && s.state === 'succeeded' && s.nominated) out.rtt = s.currentRoundTripTime;
    });
    return out;
  };
  probe.collect = function (intervalMs = 1000) {
    clearInterval(probe.timer);
    probe.timer = setInterval(async () => { const s = await probe.sample().catch(() => null); if (s) probe.series.push(s); }, intervalMs);
  };
  probe.stopCollect = function () { clearInterval(probe.timer); };

  // ---- glass-to-glass ------------------------------------------------------
  const median = (a, q) => { if (!a.length) return null; const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.floor(q * s.length))]; };
  // `wait` splits each sample at the frame's arrival (rVFC receiveTime, when the browser gives it):
  // what came before is the host, the network and the clock window, the same for every page and
  // moving between streams by itself; what came after is the page's decode and display.
  probe.g2gSummary = function () {
    const v = probe.g2g.map(s => s.ms);
    const waits = probe.g2g.filter(s => Number.isFinite(s.wait)).map(s => s.wait);
    const arrived = probe.g2g.filter(s => Number.isFinite(s.wait)).map(s => s.ms - s.wait);
    const round = x => x === null ? null : Math.round(x * 10) / 10;
    return {samples: v.length, rejected: probe.g2gRejected, median_ms: median(v, .5), p95_ms: median(v, .95), p99_ms: median(v, .99),
      min_ms: v.length ? Math.min(...v) : null, max_ms: v.length ? Math.max(...v) : null,
      waitSamples: waits.length, wait_median_ms: round(median(waits, .5)), wait_p95_ms: round(median(waits, .95)),
      wait_min_ms: waits.length ? round(Math.min(...waits)) : null, wait_max_ms: waits.length ? round(Math.max(...waits)) : null,
      arrival_median_ms: round(median(arrived, .5)), arrival_p95_ms: round(median(arrived, .95)), calibration: probe.calibration};
  };
  // Reads the clock block from every decoded frame of `video`. The block is
  // located by its magenta border once (and again whenever it is lost).
  probe.startG2g = function (video) {
    const {layout, decode} = root.RlClock;
    const find = document.createElement('canvas'), read = document.createElement('canvas');
    let box = null, readCtx = null, lost = 0;
    const isMagenta = (r, g, b) => r > 150 && b > 150 && g < 110;
    // Magenta lives elsewhere on a real desktop too: label the 4 px tiles that are mostly magenta,
    // join neighbours, and take the component shaped like the block.
    function calibrate() {
      const w = video.videoWidth, h = video.videoHeight;
      if (!w || !h) return;
      find.width = w; find.height = h;
      const ctx = find.getContext('2d', {willReadFrequently: true});
      ctx.drawImage(video, 0, 0, w, h);
      const data = ctx.getImageData(0, 0, w, h).data;
      const T = 4, tw = Math.ceil(w / T), th = Math.ceil(h / T);
      const mark = new Uint8Array(tw * th);
      for (let ty = 0; ty < th; ty++) for (let tx = 0; tx < tw; tx++) {
        let n = 0, total = 0;
        for (let y = ty * T; y < Math.min(h, ty * T + T); y++) for (let x = tx * T; x < Math.min(w, tx * T + T); x++) {
          const i = (y * w + x) * 4; total++;
          if (isMagenta(data[i], data[i + 1], data[i + 2])) n++;
        }
        if (n * 2 >= total) mark[ty * tw + tx] = 1;
      }
      const seen = new Uint8Array(tw * th);
      const wanted = layout.WIDTH / layout.HEIGHT;
      let best = null;
      for (let start = 0; start < mark.length; start++) {
        if (!mark[start] || seen[start]) continue;
        const stack = [start]; seen[start] = 1;
        let x0 = tw, y0 = th, x1 = -1, y1 = -1, count = 0;
        while (stack.length) {
          const at = stack.pop(), x = at % tw, y = (at - x) / tw;
          count++; if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y;
          for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
            const nx = x + dx, ny = y + dy;
            if (nx < 0 || ny < 0 || nx >= tw || ny >= th) continue;
            const next = ny * tw + nx;
            if (mark[next] && !seen[next]) { seen[next] = 1; stack.push(next); }
          }
        }
        const bw = (x1 - x0 + 1) * T, bh = (y1 - y0 + 1) * T;
        const ratio = bw / bh;
        if (bw < 60 || bh < 8 || Math.abs(ratio / wanted - 1) > .2) continue;
        if (!best || count > best.count) best = {count, x: x0 * T, y: y0 * T, w: bw, h: bh};
      }
      if (!best) return;
      // Tiles are coarse: the exact edge is the outermost magenta pixel near the component.
      let ex0 = w, ey0 = h, ex1 = -1, ey1 = -1;
      for (let y = Math.max(0, best.y - T); y < Math.min(h, best.y + best.h + T); y++) for (let x = Math.max(0, best.x - T); x < Math.min(w, best.x + best.w + T); x++) {
        const i = (y * w + x) * 4;
        if (isMagenta(data[i], data[i + 1], data[i + 2])) { if (x < ex0) ex0 = x; if (x > ex1) ex1 = x; if (y < ey0) ey0 = y; if (y > ey1) ey1 = y; }
      }
      box = {x: ex0, y: ey0, w: ex1 - ex0 + 1, h: ey1 - ey0 + 1};
      read.width = box.w; read.height = box.h;
      readCtx = read.getContext('2d', {willReadFrequently: true});
      probe.calibration = {...box, video: [w, h], scale: box.w / layout.WIDTH};
    }
    function bits() {
      readCtx.drawImage(video, box.x, box.y, box.w, box.h, 0, 0, box.w, box.h);
      const sx = box.w / layout.WIDTH, sy = box.h / layout.HEIGHT;
      const out = [];
      for (let i = 0; i < layout.CELLS; i++) {
        const cx = Math.round((layout.PAD + (i + .5) * layout.CELL_W) * sx), cy = Math.round(box.h / 2);
        const half = Math.max(1, Math.floor(Math.min(layout.CELL_W * sx, layout.CELL_H * sy) / 6));
        const data = readCtx.getImageData(Math.max(0, cx - half), Math.max(0, cy - half), half * 2 + 1, half * 2 + 1).data;
        let sum = 0; for (let p = 0; p < data.length; p += 4) sum += data[p] * .3 + data[p + 1] * .59 + data[p + 2] * .11;
        out.push(sum / (data.length / 4) > 128 ? 1 : 0);
      }
      return out;
    }
    let frames = 0;
    const onFrame = (now, metadata) => {
      const at = Date.now();
      // receiveTime is on performance.now()'s clock: from it to this callback, same clock.
      const wait = metadata && Number.isFinite(metadata.receiveTime) ? performance.now() - metadata.receiveTime : undefined;
      frames++;
      try {
        if (!box) { if (frames % 15 === 1) calibrate(); }
        else {
          const value = decode(bits());
          if (value === null) { probe.g2gRejected++; if (++lost > 30) { box = null; lost = 0; } }
          else {
            lost = 0;
            const ms = ((at % (1 << layout.BITS)) - value + (1 << layout.BITS)) % (1 << layout.BITS);
            // A wrong decode (a cell flipped by compression) shows as an absurd delay.
            if (ms > 2000) probe.g2gRejected++; else probe.g2g.push({ms, at, wait, presented: metadata && metadata.presentedFrames});
          }
        }
      } catch (error) { probe.g2gError = String(error); }
      video.requestVideoFrameCallback(onFrame);
    };
    video.requestVideoFrameCallback(onFrame);
  };

  // The current frame as a JPEG data URL, for the run's artifacts.
  probe.snapshot = function (video) {
    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth; canvas.height = video.videoHeight;
    canvas.getContext('2d').drawImage(video, 0, 0);
    return canvas.toDataURL('image/jpeg', .8);
  };

  // ---- per-channel tones ----------------------------------------------------
  // The tone for channel i, in Hz; the host-side player plays the same table.
  probe.toneHz = i => 500 + 200 * i;
  probe.startChannels = async function (stream, channels) {
    const context = new AudioContext({sampleRate: 48000});
    probe.audioContext = context;
    // Chromium only feeds Web Audio from a remote stream that a media element also holds.
    const element = document.createElement('audio');
    element.muted = true; element.srcObject = stream; element.play().catch(() => {});
    probe.audioElement = element;
    const source = context.createMediaStreamSource(stream);
    const splitter = context.createChannelSplitter(channels);
    source.connect(splitter);
    probe.analysers = [];
    for (let i = 0; i < channels; i++) {
      const analyser = context.createAnalyser(); analyser.fftSize = 8192; analyser.smoothingTimeConstant = 0;
      splitter.connect(analyser, i); probe.analysers.push(analyser);
    }
    await context.resume();
  };
  // The loudest frequency on each channel and its level over the floor.
  probe.measureChannels = function (tones) {
    const rate = probe.audioContext.sampleRate;
    return probe.analysers.map((analyser, channel) => {
      const data = new Float32Array(analyser.frequencyBinCount);
      analyser.getFloatFrequencyData(data);
      let peak = 0; for (let i = 1; i < data.length; i++) if (data[i] > data[peak]) peak = i;
      const sorted = Array.from(data).sort((a, b) => a - b);
      const floorDb = Math.round(sorted[Math.floor(sorted.length / 2)]);
      // The level at each expected tone, to tell a clean channel from a mixed one.
      const at = (tones || []).map(hz => {
        const bin = Math.round(hz * analyser.fftSize / rate);
        return Math.round(Math.max(data[bin - 1], data[bin], data[bin + 1]));
      });
      return {channel, peakHz: Math.round(peak * rate / analyser.fftSize), peakDb: Math.round(data[peak]), floorDb, at};
    });
  };

  root.RlProbe = probe;
})(typeof globalThis !== 'undefined' ? globalThis : this);
