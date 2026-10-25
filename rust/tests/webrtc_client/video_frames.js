// Runs in a worker, as the RTCRtpScriptTransform of the video receiver (client.js, `dumpVideo`).
// Hands the first encoded frames to the page so the harness can run ffprobe on the bitstream the
// host sent: bit depth, primaries, transfer function and HDR metadata are in the stream, and a
// client's decoder reports none of them. Every frame is passed on untouched.
onrtctransform = event => {
  const {readable, writable} = event.transformer;
  const keep = event.transformer.options.frames;
  let seen = 0;
  readable.pipeThrough(new TransformStream({
    transform(frame, controller) {
      if (seen < keep) {
        seen++;
        postMessage({type: 'frame', key: frame.type === 'key', data: frame.data.slice(0)});
      }
      controller.enqueue(frame);
    },
  })).pipeTo(writable);
};
