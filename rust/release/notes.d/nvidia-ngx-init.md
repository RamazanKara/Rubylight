# RTX check
- **The check of RTX HDR and RTX Video enhancement no longer crashes.** On GeForce RTX cards the app page said "NVIDIA's check crashed (0xc0000005)" and both stayed off (issue #103): the call that starts NVIDIA's library left out one argument, so the library read the version number as an address. The call now matches NVIDIA's library.
