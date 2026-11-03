## en
- Windows: a resumed stream no longer drops a VRR monitor to V-Sync. The display check waits for the frame rate to settle, and no longer calls a monitor it saw waiting for frames fixed-refresh without testing a frame on it (#72).
- Windows: a stream the display check turned to V-Sync checks the display again now and then, and returns to presenting on arrival once frames restart the display's refresh.
