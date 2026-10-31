# WebRTC send batching
- **WebRTC streams hand each frame to the network 2.2-2.7x faster, with 5-12% less host CPU.** Rubylight now writes each run of equal-sized packets to Windows in one call instead of one call per packet (measured on this PC and the 780M laptop).
