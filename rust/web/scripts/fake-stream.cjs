// A fake host for the play and guest pages' screenshots (scripts/screenshots.cjs, SHOT_STREAM).
// `install` runs in the page before its scripts: it replaces WebSocket and RTCPeerConnection with
// ones that answer as a host would, so each screen shows without a host, a stream or a network.
// The picture is a canvas drawn in the page. Nothing here ships: only the screenshot script loads it.
//
// SHOT_STREAM for /play/: pair (the PIN waits), pair-failed, library, error, stream.
// For /guest/#<token>: name, waiting, expired, stream.
//
// Both pages stop where a person acts, so a shot past that point needs the act:
//   /play/   library and stream both open on the games. SHOT_KEYS=Enter,Enter plays the game in focus
//            (the first press puts the keyboard on its tile, the second starts it); only then does
//            `stream` show the picture. SHOT_SETTLE=3000 lets the stats capsule take its second look.
//            For the dock, start a game by its tile instead, SHOT_CLICK="Hades II" SHOT_HOLD=Escape:
//            the script clicks, then holds, then presses SHOT_KEYS, and a name with a comma in it
//            ("Portal 2, running") cannot be clicked that way.
//   /guest/  waiting and stream open on the name card with "Sam" filled in. SHOT_CLICK=Join sends
//            the name; only then does the page wait for the host or show the stream.

/** Runs in the page. `state` is SHOT_STREAM, `pad` SHOT_PAD. */
function install({ state, pad }) {
  const later = (run, ms = 0) => setTimeout(run, ms);
  const GAMES = [
    { id: 'portal', title: 'Portal 2', artworkAvailable: true, running: true },
    { id: 'hades', title: 'Hades II', artworkAvailable: true, running: false },
    { id: 'forza', title: 'Forza Horizon 5', artworkAvailable: false, running: false },
    { id: 'witcher', title: 'The Witcher 3: Wild Hunt', artworkAvailable: true, running: false },
    { id: 'celeste', title: 'Celeste', artworkAvailable: false, running: false },
    { id: 'desktop', title: 'Desktop', artworkAvailable: false, running: false },
    { id: 'steam', title: 'Steam Big Picture', artworkAvailable: true, running: false },
    { id: 'elden', title: 'Elden Ring', artworkAvailable: true, running: false },
  ];
  const CAPABILITIES = {
    videoModes: [{ width: 1920, height: 1080, frameRates: [60], codecs: ['h264', 'hevc', 'av1'], defaultBitrateKbps: 20000 }],
    extensions: { input: { label: 'input', version: 1, keyboard: true, mouse: true } },
  };

  /** A painted picture: a cover for a game, or the "game" itself. */
  function paint(width, height, hue, text) {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const c = canvas.getContext('2d');
    const sky = c.createLinearGradient(0, 0, width, height);
    sky.addColorStop(0, `hsl(${hue} 70% 46%)`);
    sky.addColorStop(1, `hsl(${(hue + 60) % 360} 60% 14%)`);
    c.fillStyle = sky;
    c.fillRect(0, 0, width, height);
    c.fillStyle = 'rgb(255 255 255 / 14%)';
    for (let i = 0; i < 6; i++) c.fillRect((i * width) / 6, height * (0.55 + 0.06 * Math.sin(i * 2)), width / 7, height);
    c.fillStyle = 'rgb(255 255 255 / 92%)';
    c.font = `700 ${Math.round(height / 9)}px "Segoe UI", sans-serif`;
    c.fillText(text, width * 0.08, height * 0.2);
    return canvas;
  }
  const hueOf = (title) => [...title].reduce((sum, char) => (sum * 31 + char.codePointAt(0)) % 360, 0);

  class FakeSocket {
    constructor(url, protocols) {
      this.url = String(url);
      this.protocol = [].concat(protocols)[0] ?? '';
      this.readyState = 0;
      this.guest = this.url.includes('/guest/');
      later(() => {
        if (this.guest && state === 'expired') return this.onerror?.({});
        if (!this.guest && state === 'error') return (this.onerror?.({}), this.onclose?.({ reason: '' }));
        this.readyState = 1;
        this.onopen?.({});
        if (!this.guest) this.say({ type: 'auth-required', nonce: 'a0'.repeat(32) });
      });
    }
    say(message) {
      later(() => this.onmessage?.({ data: JSON.stringify({ version: 2, ...message }) }));
    }
    signedIn() {
      this.say({ type: 'gateway-status', gatewayName: 'Gaming PC' });
      this.say({ type: 'capabilities', ...CAPABILITIES });
    }
    send(text) {
      const message = JSON.parse(text);
      switch (message.type) {
        case 'request-pairing':
          if (state === 'pair') return;
          if (state === 'pair-failed') return this.say({ type: 'error', requestType: 'request-pairing', code: 'incorrect-pin', message: 'incorrect-pin' });
          this.say({ type: 'paired', clientId: '00112233445566778899aabbccddeeff', clientSecret: '0f'.repeat(32) });
          return this.signedIn();
        case 'authenticate':
          this.say({ type: 'authenticated' });
          return this.signedIn();
        case 'get-apps':
          return this.say({ type: 'apps', apps: GAMES });
        case 'get-app-artwork': {
          const game = GAMES.find((g) => g.id === message.appId);
          const data = paint(300, 400, hueOf(game.title), '').toDataURL('image/png').split(',')[1];
          return this.say({ type: 'app-artwork', appId: message.appId, available: true, mimeType: 'image/png', data });
        }
        case 'start-session':
          return this.say({ type: 'offer', sessionId: 'shot', sdp: 'v=0\r\n' });
        case 'join':
          if (state === 'waiting') {
            return this.say({ type: 'error', code: 'not-streaming', message: 'The host is not streaming right now. This page joins when the stream starts.' });
          }
          return this.say({ type: 'offer', sdp: 'v=0\r\n', name: message.name || 'Guest', permission: 'controller-keyboard-mouse', codec: 'HEVC', width: 1920, height: 1080, fps: 60 });
        case 'answer':
          if (this.guest) this.say({ type: 'streaming' });
          return;
        default:
      }
    }
    close() {
      this.readyState = 3;
    }
  }

  class FakeChannel {
    constructor(label) {
      this.label = label;
      this.readyState = 'open';
      later(() => this.onopen?.({}));
    }
    send() {}
    close() {}
  }

  class FakePeer {
    constructor() {
      this.connectionState = 'new';
      this.localDescription = null;
    }
    async setRemoteDescription() {
      // The "game": a painted frame as a real video track, so the page plays a picture.
      const canvas = paint(1920, 1080, 205, 'Portal 2');
      const track = canvas.captureStream(1).getVideoTracks()[0];
      later(() => this.ontrack?.({ track, receiver: {} }));
    }
    async createAnswer() {
      return { type: 'answer', sdp: 'v=0\r\na=rtpmap:111 opus/48000/2\r\n' };
    }
    async setLocalDescription(description) {
      this.localDescription = description;
      later(() => {
        this.connectionState = 'connected';
        this.onconnectionstatechange?.({});
      }, 20);
    }
    createDataChannel(label) {
      return new FakeChannel(label);
    }
    async getStats() {
      const seconds = performance.now() / 1000;
      return new Map([
        ['v', { type: 'inbound-rtp', kind: 'video', codecId: 'c', bytesReceived: Math.round(seconds * 2_350_000), framesDecoded: Math.round(seconds * 60), frameWidth: 1920, frameHeight: 1080 }],
        ['c', { type: 'codec', id: 'c', mimeType: 'video/H265' }],
        ['p', { type: 'candidate-pair', state: 'succeeded', nominated: true, currentRoundTripTime: 0.004, responsesReceived: 3 }],
      ]);
    }
    close() {
      this.connectionState = 'closed';
    }
  }

  window.WebSocket = FakeSocket;
  window.RTCPeerConnection = FakePeer;
  // An expired link: the guest port answers 404 to the page's own address.
  if (state === 'expired') window.fetch = async () => ({ status: 404 });
  // A pad in the player's hands: its right stick rests off centre, which moves nothing on the page.
  if (pad) {
    const id = { xbox: 'Xbox Wireless Controller (STANDARD GAMEPAD)', playstation: 'DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)', nintendo: 'Pro Controller (STANDARD GAMEPAD Vendor: 057e Product: 2009)' }[pad];
    const held = { index: 0, id, mapping: 'standard', connected: true, timestamp: 0, buttons: Array.from({ length: 17 }, () => ({ pressed: false, value: 0 })), axes: [0, 0, 0.55, 0] };
    navigator.getGamepads = () => [held];
  }
  // A paired browser for every state past pairing.
  if (!state.startsWith('pair')) {
    localStorage.setItem('rubylight.play.device', JSON.stringify({ clientId: '00112233445566778899aabbccddeeff', secret: '0f'.repeat(32) }));
  }
  localStorage.setItem('rubylight.play.stats', '1');
  if (state !== 'name') localStorage.setItem('rubylight.guest.name', 'Sam');
}

module.exports = { install };
