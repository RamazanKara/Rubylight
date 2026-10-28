# Bug hunt: host fixes 2
- After a crash, display recovery keeps the frame limiter, audio, display activation and layout restores that failed and tries them again at the next start, instead of forgetting them.
- Resetting display persistence in the web UI is refused while a stream is starting, running or ending.
- After a crash on a host whose only display was the virtual one, display recovery no longer fails at every start and slows every stream end.
- A Steam Streaming Microphone install that fails is tried again on a later connection (at most every 2 minutes) instead of never again until the host restarts.
- Ending a stream while its microphone device is still being installed or opened no longer holds up other clients' microphones.
- 7.1.4 audio played on 7.1, 5.1 or stereo speakers keeps the height channels, mixed into the speakers below them, instead of dropping them.
- Turning USB forwarding off in the web UI detaches every forwarded device right away, not only when its stream ends.
- Image clipboard requests take turns, so several at once can no longer add up to gigabytes of memory; a device's second one while its first still runs gets 429.
