## en
- Windows: a resumed stream no longer drops a VRR monitor to V-Sync. The display check waits for the stream to present at its own rate before it calls a display fixed-refresh, and a display it saw waiting for a present is never called that (#72).
- Windows: a stream the display check turned to V-Sync checks the display again now and then and returns to presenting on arrival once the display refreshes on present.
