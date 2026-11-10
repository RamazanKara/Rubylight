## desktop
- PyroWave GPU decoding: the decoder thread keeps track of the records it has handed to the GPU decoder with a bitmap instead of a hash set, which saves work on every network packet of a frame.
- The PyroWave and video decoder stage times in the log no longer round each step down to a whole microsecond. The PyroWave push stage, which is timed once per network packet, read lower than it was before.
