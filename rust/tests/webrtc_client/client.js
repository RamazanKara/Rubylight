// Our own protocol v2 client, in plain JS for the e2e browser. Unlike the
// browser client at its pin it can force any codec and ask for 5.1 or 7.1
// through the surround extension (`audio.channels` 6 or 8), which is what the
// AV1, HEVC and surround legs need. Measurement is RlProbe's (probe.js).
(function (root) {
  // libwebrtc's own multiopus parameters for 5.1 and 7.1.
  const SURROUND = {
    6: {mapping: '0,4,1,2,3,5', streams: 4, coupled: 2},
    8: {mapping: '0,6,1,2,3,4,5,7', streams: 5, coupled: 3},
  };
  const status = text => { const el = document.getElementById('status'); if (el) el.textContent = text; };

  class Client {
    constructor() { this.channels = {}; this.log = []; this.states = []; this.candidates = 0; }
    note(text) { this.log.push({at: Date.now(), text}); status(text); }

    // Signs in, starts a session and answers the host's offer; resolves once
    // the peer connection is connected. `options`: url, clientId, secret,
    // appId (default: the first app), codec, width, height, fps,
    // bitrateKbps, channels, hdr, dumpVideo (how many encoded frames to keep for ffprobe).
    async start(options) {
      const o = {width: 1920, height: 1080, fps: 60, bitrateKbps: 30000, channels: 2, hdr: false, codec: 'h264', ...options};
      const sig = this.sig = new root.RlSignalling.Signalling(o.url);
      await sig.open();
      const {capabilities} = await sig.authenticate(o.clientId, o.secret);
      this.capabilities = capabilities;
      let fail;
      const failed = new Promise((_, reject) => { fail = reject; });
      failed.catch(() => {});
      sig.on(message => {
        if (message.type === 'error') fail(new Error(message.code + ': ' + message.message));
        if (message.type === 'session-status') this.states.push({at: Date.now(), state: message.state, message: message.message});
        if (message.type === 'candidate' && this.pc) {
          this.pc.addIceCandidate({candidate: message.candidate, sdpMid: message.mid}).catch(error => this.note('addIceCandidate: ' + error));
        }
      });
      const appId = o.appId || (await sig.apps())[0].id;
      sig.send('start-session', {appId,
        video: {width: o.width, height: o.height, fps: o.fps, codec: o.codec, bitrateKbps: o.bitrateKbps, hdr: o.hdr},
        audio: {channels: o.channels}});
      const offer = await sig.expect('offer', 60000);
      this.sessionId = offer.sessionId;
      this.offerSdp = offer.sdp;
      const pc = this.pc = new RTCPeerConnection({iceServers: []});
      const tracks = new Promise(resolve => {
        const seen = {};
        pc.ontrack = event => {
          seen[event.track.kind] = event;
          if (event.track.kind === 'video') {
            const video = document.getElementById('video');
            video.srcObject = event.streams[0] || new MediaStream([event.track]);
            // The host sends frames the moment they exist: no playout delay, like the reference client.
            try { event.receiver.jitterBufferTarget = 0; } catch {}
            if (o.dumpVideo) this.keepVideoFrames(event.receiver, o.dumpVideo);
          }
          if (event.track.kind === 'audio' && SURROUND[o.channels]) this.decodeAudio(event.receiver, o.channels);
          if (seen.video && seen.audio) resolve(seen);
        };
        setTimeout(() => resolve(seen), 20000);
      });
      pc.ondatachannel = event => {
        const channel = event.channel; this.channels[channel.label] = channel;
        channel.onmessage = message => { (channel.received ||= []).push({at: Date.now(), data: typeof message.data === 'string' ? message.data.slice(0, 200) : '<binary>'}); };
        channel.onopen = () => this.note('data channel open: ' + channel.label);
      };
      pc.onicecandidate = event => {
        if (event.candidate) { this.candidates++; sig.send('candidate', {sessionId: this.sessionId, candidate: event.candidate.candidate, mid: event.candidate.sdpMid || '0'}); }
      };
      const connected = new Promise((resolve, reject) => {
        pc.onconnectionstatechange = () => {
          this.note('connection ' + pc.connectionState);
          if (pc.connectionState === 'connected') resolve();
          if (pc.connectionState === 'failed') reject(new Error('the peer connection failed'));
        };
      });
      // The offer is always standard stereo Opus: Chromium refuses `multiopus` in a remote
      // offer, but keeps it in a local description it was munged into. So a surround client
      // edits only its own answer, to multiopus with Chromium's own 5.1 and 7.1 parameters.
      // A stereo client puts `stereo=1` in its answer, because Chromium decodes from its own
      // description and a stereo stream plays as one channel without it.
      await pc.setRemoteDescription({type: 'offer', sdp: offer.sdp});
      const answer = await pc.createAnswer();
      const opus = /^a=rtpmap:(\d+) opus\/48000\/2/m.exec(answer.sdp);
      if (!opus) throw new Error('the answer has no opus/48000/2');
      const fmtp = new RegExp('^a=fmtp:' + opus[1] + ' [^\r\n]*', 'm');
      const surround = SURROUND[o.channels];
      if (surround) {
        answer.sdp = answer.sdp.replace(opus[0], 'a=rtpmap:' + opus[1] + ' multiopus/48000/' + o.channels)
          .replace(fmtp, 'a=fmtp:' + opus[1] + ' channel_mapping=' + surround.mapping + ';num_streams=' + surround.streams + ';coupled_streams=' + surround.coupled + ';minptime=10;useinbandfec=0');
      } else {
        answer.sdp = answer.sdp.replace(fmtp, line => /stereo=1/.test(line) ? line : line + ';stereo=1;sprop-stereo=1');
      }
      await pc.setLocalDescription(answer);
      this.answerSdp = answer.sdp;
      sig.send('answer', {sessionId: this.sessionId, sdp: pc.localDescription.sdp});
      let timer;
      await Promise.race([connected, failed,
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('the peer connection did not connect in 60 s')), 60000); })]);
      clearTimeout(timer);
      this.tracks = await tracks;
      if (this.tracks.audio) {
        await root.RlProbe.startChannels(this.tracks.audio.streams[0] || new MediaStream([this.tracks.audio.track]), o.channels);
      }
      return {sessionId: this.sessionId, hasVideo: !!this.tracks.video, hasAudio: !!this.tracks.audio};
    }

    /** The first `count` encoded video frames, kept for `videoDump()`. */
    keepVideoFrames(receiver, count) {
      this.videoFrames = [];
      const worker = new Worker('video_frames.js');
      worker.onmessage = ({data}) => { if (data.type === 'frame') this.videoFrames.push({key: data.key, data: new Uint8Array(data.data)}); };
      receiver.transform = new RTCRtpScriptTransform(worker, {frames: count});
    }

    /** The kept frames, one after the other, as base64 (Annex B for H.264 and HEVC, OBUs for AV1). */
    async videoDump() {
      const frames = this.videoFrames || [];
      const url = await new Promise(resolve => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result);
        reader.readAsDataURL(new Blob(frames.map(f => f.data)));
      });
      return {frames: frames.length, keyFrames: frames.filter(f => f.key).length, sizes: frames.map(f => f.data.length), base64: url.split(',')[1] || ''};
    }

    /** Asks for a keyframe on the control channel every `ms` until `stopKeyframes()`: the recovery a
     * client with losses would trigger, at a rate that keeps the pacer's burst handling in use. */
    requestKeyframes(ms) {
      this.keyframeRequests = 0;
      this.keyframeTimer = setInterval(() => {
        const channel = this.channels.control;
        if (channel && channel.readyState === 'open') { channel.send('{"type":"request-keyframe"}'); this.keyframeRequests++; }
      }, ms);
    }
    stopKeyframes() { clearInterval(this.keyframeTimer); return this.keyframeRequests || 0; }

    /** The encoded surround packets, decoded in a worker with the mapping this client answered. */
    decodeAudio(receiver, channels) {
      const layout = SURROUND[channels];
      this.decoded = {levels: [], errors: [], support: null};
      const worker = new Worker('frames.js');
      worker.onmessage = ({data}) => {
        if (data.type === 'levels') { this.decoded.levels.push(data.levels); this.decoded.decodedChannels = data.decodedChannels; }
        else if (data.type === 'error') this.decoded.errors.push(data.message);
        else if (data.type === 'support') this.decoded.support = data;
      };
      worker.onerror = e => this.decoded.errors.push(e.message || 'worker error');
      receiver.transform = new RTCRtpScriptTransform(worker, {
        channels, streams: layout.streams, coupled: layout.coupled, mapping: layout.mapping.split(',').map(Number),
        tones: Array.from({length: channels}, (_, c) => root.RlProbe.toneHz(c)),
      });
    }

    stop(quitApp = true) {
      try { this.sig.send('stop-session'); } catch {}
      if (quitApp) try { this.sig.send('stop-host-session'); } catch {}
    }
  }

  root.RlClient = new Client();
})(globalThis);
