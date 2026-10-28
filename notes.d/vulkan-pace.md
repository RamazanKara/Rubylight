## en
- When "Render through Vulkan" can't keep up on a display (it shows under 80% of the received frames, or of the display's refreshes when the stream runs faster, for 5 seconds), a message says so once per PC and suggests turning it off. The setting stays as you set it.
- "Why did my stream stutter?" now takes its once-a-second sample off the app's main thread, so a hanging decoder can no longer freeze the app.
