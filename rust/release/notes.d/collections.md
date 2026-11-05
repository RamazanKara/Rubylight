# Library
- **Collections without setup.** Each library entry now names its Steam collections and Playnite categories, so Nightjar can group Home by them. Steam's files are only read; missing or changed files mean no collections and a log line, never a failed library. A file that can't be read keeps the last collections for two more scans at most.
- **Steam installs show up sooner.** With auto sync on, an install or removal reaches Nightjar in about a minute instead of up to 5 minutes. Library scans now run at background priority.
