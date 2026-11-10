# Less background work
- **Listing the running programs takes a third of the time.** The host reads the process list in one call and opens a program only once: 1.7 ms instead of 5.6 ms per list on our RX 7900 XT PC with 370 programs running. The host lists them while it tracks a game.
- The 30-second library check looks up where Steam is installed once instead of twice.
- Full chroma's description no longer says "on AMD": the setting was never limited to one GPU vendor.
- With no stream, launch or game running, the host's housekeeping wakes when there is something to do instead of checking constantly, and streams, control messages, UPnP port forwarding, the library sync and the PC's name lookups each do less repeated work.
