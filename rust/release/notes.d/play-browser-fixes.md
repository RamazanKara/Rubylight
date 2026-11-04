# Browser play fixes
- **No green rows under AV1 in the browser.** On AMD the encoder pads 1080p AV1 to 1082 rows and browsers show them; the Play, Stream and guest pages now crop them, and the pointer maps to the picture without them.
- **Holding Back or Esc opens the play menu on a real keyboard or remote.** The key's repeats closed the menu the moment it opened.
