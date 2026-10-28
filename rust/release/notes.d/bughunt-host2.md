# Bug hunt: host fixes 2
- After a crash, display recovery keeps the frame limiter, audio, display activation and layout restores that failed and tries them again at the next start, instead of forgetting them.
- Resetting display persistence in the web UI is refused while a stream is starting, running or ending.
- After a crash on a host whose only display was the virtual one, display recovery no longer fails at every start and slows every stream end.
