// Protocol v2 signalling for the WebRTC test client, in Node and the browser.
// JSON over a WebSocket, every message with "version":2 and a "type"
// (Monlight-Tizen-WebRtc-Client docs/protocol.md at the pin in
// docs/webrtc-design.md). Plain JS: no dependencies, no build.
(function (root) {
  const encoder = new TextEncoder();
  const hex = bytes => Array.from(new Uint8Array(bytes), b => b.toString(16).padStart(2, '0')).join('');

  // HMAC-SHA256(secret bytes, "moonlight-webrtc-tv-auth-v1:" + nonce hex), as
  // hex; the secret is the 64 hex digits the host issued, decoded.
  async function proof(secret, nonce) {
    const bytes = Uint8Array.from(secret.match(/../g), pair => parseInt(pair, 16));
    const key = await crypto.subtle.importKey('raw', bytes, {name: 'HMAC', hash: 'SHA-256'}, false, ['sign']);
    return hex(await crypto.subtle.sign('HMAC', key, encoder.encode('moonlight-webrtc-tv-auth-v1:' + nonce)));
  }

  class Signalling {
    constructor(url) { this.url = url; this.queue = []; this.waiters = []; this.log = []; this.closed = false; this.listeners = []; }

    open() {
      return new Promise((resolve, reject) => {
        const ws = this.ws = new WebSocket(this.url);
        ws.onopen = () => resolve(this);
        ws.onerror = () => reject(new Error('cannot connect to ' + this.url));
        ws.onclose = () => { this.closed = true; this.flush(); };
        ws.onmessage = event => {
          const message = JSON.parse(event.data);
          this.log.push({at: Date.now(), message: message.type});
          for (const listener of this.listeners) listener(message);
          this.queue.push(message); this.flush();
        };
      });
    }
    send(type, fields = {}) { this.ws.send(JSON.stringify({version: 2, type, ...fields})); }
    close() { try { this.ws.close(); } catch {} }
    on(listener) { this.listeners.push(listener); }

    flush() {
      for (const waiter of this.waiters.slice()) {
        const index = this.queue.findIndex(waiter.match);
        if (index >= 0) { this.waiters.splice(this.waiters.indexOf(waiter), 1); clearTimeout(waiter.timer); waiter.resolve(this.queue.splice(index, 1)[0]); }
        else if (this.closed) { this.waiters.splice(this.waiters.indexOf(waiter), 1); clearTimeout(waiter.timer); waiter.reject(new Error(`the connection closed while waiting for ${waiter.what}`)); }
      }
    }
    // The next message of `type` (or, if it is an array, any of them);
    // an `error` message rejects when it is not itself wanted.
    expect(type, timeout = 15000) {
      const types = Array.isArray(type) ? type : [type];
      const match = m => types.includes(m.type) || m.type === 'error';
      return new Promise((resolve, reject) => {
        const waiter = {match, what: types.join('/'), reject, resolve: message => {
          if (message.type === 'error' && !types.includes('error')) reject(Object.assign(new Error(`${message.code}: ${message.message}`), {code: message.code, reply: message}));
          else resolve(message);
        }};
        waiter.timer = setTimeout(() => { this.waiters.splice(this.waiters.indexOf(waiter), 1); reject(new Error(`timed out waiting for ${waiter.what}`)); }, timeout);
        this.waiters.push(waiter); this.flush();
      });
    }

    async authRequired() { const m = await this.expect('auth-required'); this.nonce = m.nonce; return m; }
    // Signs in and returns {gateway, capabilities}.
    async authenticate(clientId, secret) {
      await this.authRequired();
      this.send('authenticate', {clientId, proof: await proof(secret, this.nonce)});
      await this.expect('authenticated');
      const gateway = await this.expect('gateway-status');
      const capabilities = await this.expect('capabilities');
      return {gateway, capabilities};
    }
    // Asks to pair with `pin`; resolves with the `paired` message once the
    // console approves it. `approve` runs after the request is sent.
    async pair(pin, clientName, approve) {
      await this.authRequired();
      this.send('request-pairing', {pin, clientName});
      const outcome = this.expect('paired', 30000);
      if (approve) await approve();
      return outcome;
    }
    async apps() { this.send('get-apps'); return (await this.expect('apps')).apps; }
    async artwork(appId) { this.send('get-app-artwork', {appId}); return this.expect('app-artwork'); }
  }

  const api = {Signalling, proof, hex};
  if (typeof module !== 'undefined' && module.exports) module.exports = api; else root.RlSignalling = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
