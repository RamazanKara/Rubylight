## en
- On desktop, a resumed stream on a VRR monitor stays on VRR at the stream's own frame rate. The display check judged it by the rate of the stream's first second, called the monitor fixed-refresh half a second in, and the stream stayed on V-Sync with higher display times. A fixed-refresh monitor under a slower stream still turns to V-Sync.
- The "Audio arrival" log line no longer reads almost every packet as 8 to 15 ms late when a few come early, as they do about twice a second from Windows' audio engine on the PC. Real stalls still show.
