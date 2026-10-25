// A standard gamepad for headless browsers, which have none. The stick moves
// slowly so the page's input loop keeps sending snapshots, and every rumble
// effect the host asks for is recorded in window.__rumbles.
(function (root) {
  const rumbles = root.__rumbles = [];
  const pad = {
    id: 'Xbox 360 Controller (STANDARD GAMEPAD Vendor: 045e Product: 028e)', index: 0, connected: true, mapping: 'standard', timestamp: 0,
    buttons: Array.from({length: 17}, () => ({pressed: false, touched: false, value: 0})),
    axes: [0, 0, 0, 0],
    vibrationActuator: {
      type: 'dual-rumble', effects: ['dual-rumble'],
      playEffect(type, params) { rumbles.push({at: Date.now(), type, ...params}); return Promise.resolve('complete'); },
      reset() { rumbles.push({at: Date.now(), reset: true}); return Promise.resolve('complete'); },
    },
  };
  root.__pad = pad;
  navigator.getGamepads = () => {
    pad.timestamp = performance.now();
    pad.axes[0] = Math.sin(performance.now() / 700) * .4;
    pad.buttons[0].pressed = (Math.floor(performance.now() / 1500) % 2) === 0; pad.buttons[0].value = pad.buttons[0].pressed ? 1 : 0;
    return [pad, null, null, null];
  };
})(globalThis);
