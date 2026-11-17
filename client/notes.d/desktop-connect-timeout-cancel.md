## desktop
- Cancel on the connecting screen returns to the PC list at once, even while the graphics driver is stuck. A start that stops answering ends by itself after five minutes and says the graphics driver or a decoder stopped answering.
- The decoder checks before a start run on a worker thread, so the window and Cancel keep answering while a driver reset holds one; the packed check's decodes moved there too, its draw stays on the renderer's thread.
