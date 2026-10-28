## en
- When "Render through Vulkan" can't keep up on a display (it shows under 80% of the received frames, or of the display's refreshes when the stream runs faster, for 5 seconds), a message says so once per PC and suggests turning it off. The setting stays as you set it.
- "Why did my stream stutter?" now takes its once-a-second sample off the app's main thread, so a hanging decoder can no longer freeze the app.

## de
- Kommt „Über Vulkan rendern“ auf einem Bildschirm nicht mit (es zeigt 5 Sekunden lang weniger als 80 % der empfangenen Bilder, oder der Bildwiederholungen des Bildschirms, wenn der Stream schneller läuft), sagt eine Meldung das einmal pro PC und schlägt vor, es auszuschalten. Die Einstellung bleibt, wie du sie gesetzt hast.
- „Warum hat der Stream geruckelt?“ misst jetzt sekündlich außerhalb des Hauptthreads der App, sodass ein hängender Decoder die App nicht mehr einfrieren kann.
