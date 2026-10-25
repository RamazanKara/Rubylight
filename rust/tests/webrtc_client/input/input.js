// Keyboard, mouse and gamepad senders for the WebRTC input leg, in the page that RlClient streams
// to. The `input` DataChannel is client-opened (docs/webrtc-design.md, "Keyboard and mouse"); the
// gamepad announcement goes on the host's `control` channel and the snapshots on `gamepad`.
(function (root) {
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const PAD_ID = 'Xbox 360 Controller (STANDARD GAMEPAD Vendor: 045e Product: 028e)';
  const input = {channel: null, closes: 0, seq: 0, messages: []};

  // Opens the `input` channel (ordered, reliable by default) on the connected peer.
  input.open = async function (timeout = 10000) {
    const pc = root.RlClient.pc;
    const channel = this.channel = pc.createDataChannel('input', {ordered: true});
    channel.received = [];
    channel.onmessage = event => channel.received.push({at: Date.now(), data: String(event.data).slice(0, 200)});
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('the input channel did not open in ' + timeout + ' ms')), timeout);
      channel.onopen = () => { clearTimeout(timer); resolve(); };
      channel.onclose = () => { clearTimeout(timer); reject(new Error('the input channel closed while opening')); };
    });
    return {label: channel.label, id: channel.id, ordered: channel.ordered, maxRetransmits: channel.maxRetransmits, maxPacketLifeTime: channel.maxPacketLifeTime};
  };

  input.send = function (message) {
    const text = JSON.stringify({v: 1, ...message});
    this.channel.send(text);
    this.messages.push(message);
  };

  // Runs a list of steps: a number sleeps that many ms, anything else is sent as an input message.
  input.run = async function (steps, gapMs = 6) {
    for (const step of steps) {
      if (typeof step === 'number') await sleep(step);
      else { this.send(step); if (gapMs) await sleep(gapMs); }
    }
  };

  input.close = function () { this.closes++; this.channel.close(); return this.channel.readyState; };

  // ---- gamepad -------------------------------------------------------------------------------------
  const control = () => root.RlClient.channels.control;
  const pad = () => root.RlClient.channels.gamepad;

  input.channelsOpen = function () {
    const c = root.RlClient.channels;
    return {control: c.control ? c.control.readyState : null, gamepad: c.gamepad ? c.gamepad.readyState : null};
  };

  input.announce = function (controllerId = 0) {
    control().send(JSON.stringify({v: 2, type: 'gamepad-connected', controllerId, id: PAD_ID, mapping: 'standard',
      buttons: 17, axes: 4, dualRumble: true, triggerRumble: false}));
  };
  input.unplug = function (controllerId = 0) {
    control().send(JSON.stringify({v: 2, type: 'gamepad-disconnected', controllerId}));
  };

  // Sends a snapshot on the `gamepad` channel, with the next sequence number.
  input.state = function (state, controllerId = 0) {
    pad().send(JSON.stringify({v: 2, type: 'gamepad-state', controllerId, seq: ++this.seq, buttons: 0, leftTrigger: 0, rightTrigger: 0,
      leftStickX: 0, leftStickY: 0, rightStickX: 0, rightStickY: 0, ...state}));
  };

  // Holds a state for `ms`, re-sending it every `every` ms like a client's polling loop.
  input.hold = async function (state, ms, every = 50) {
    const end = performance.now() + ms;
    do { this.state(state); await sleep(every); } while (performance.now() < end);
  };

  // Everything the host sent back on the control channel, parsed.
  input.controlMessages = function () {
    const channel = root.RlClient.channels.control;
    return (channel && channel.received || []).map(entry => { try { return {at: entry.at, ...JSON.parse(entry.data)}; } catch { return {at: entry.at, raw: entry.data}; } });
  };

  root.RlInput = input;
})(globalThis);
