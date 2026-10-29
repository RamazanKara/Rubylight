## en
- **PC render resolution and PC display scaling are separate settings on the desktop app.** The old "PC display scaling" slider set how large the PC renders, so 200 % streamed a 4K picture from an 8K virtual display. It is now called PC render resolution. The new PC display scaling sets the text and interface size of a Rubylight virtual display without changing its resolution.
- **PC display scaling on Android too.** It sits below PC render resolution in the Display settings, from the PC's own scale up to 500 %, and each PC's settings can choose its own.
- PC display scaling needs a Rubylight host newer than 3.0.0-rc.4. Older hosts would ignore it, so Android shows it as unavailable in that PC's settings and neither app sends it.
- If you raised the old desktop slider above 100 % to get bigger text, set PC render resolution back to 100 % and use PC display scaling instead.
