## en
- New setting **Group incoming video packets** (Settings → Streaming → Connection, on by default): turn it off if the picture turns blocky or black at some bitrates.

## desktop
- Windows: fixed blocky and then black video at some bitrates with network adapters that merge incoming packets without reporting their size, such as some USB and dock Ethernet adapters (nightjar#103). Nightjar now notices this and stops merging packets for the rest of the stream; **Group incoming video packets** (Settings → Streaming → Connection) turns it off from the start.
