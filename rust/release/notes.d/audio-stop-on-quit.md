# Audio stop on quit
- Quitting a stream no longer logs a false "Audio stopped (client audio ping timed out)" warning just before the client disconnects. A client whose audio really goes quiet during a stream still gets the warning.
