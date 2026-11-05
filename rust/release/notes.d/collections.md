# Library
- **Collections without setup.** Each library entry now names its Steam collections and Playnite categories, so Nightjar can group Home by them. Steam's files are only read; missing or changed files mean no collections and one log line, never a failed library.
- **Steam installs show up sooner.** An install or removal reaches the library within about 30 seconds instead of up to 5 minutes. Library scans now run at background priority, so they stay out of a running stream's way.
