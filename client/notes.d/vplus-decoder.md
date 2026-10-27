## en
- Snapdragon decoders get extra low-latency options when they support them.
- MediaTek decoders ask for shorter queues first and fall back to the longer ones if that setup fails.
- HEVC on newer Amlogic TV boxes (not Fire TV) no longer uses the low-latency decoder flags.
- **HEVC low-latency flags** is now Auto, On or Off. Auto leaves them out on newer Amlogic boxes only, On uses them there too, and Off leaves them out everywhere, as the older switch did.
- The debug overlay shows frame pacing: average interval, jitter, judder, how many vsyncs each frame took and a timeline of the last frames.
- A frame too large for the decoder's buffer restarts the decoder with a bigger one instead of ending the stream.
- A controller gyroscope that only reports zeros switches player 1 to the device's motion sensors.

## de
- Snapdragon-Decoder bekommen zusätzliche Low-Latency-Optionen, wenn sie diese unterstützen.
- MediaTek-Decoder fordern zuerst kürzere Warteschlangen an und greifen auf die längeren zurück, wenn diese Einrichtung fehlschlägt.
- HEVC auf neueren Amlogic-TV-Boxen (nicht Fire TV) verwendet die Low-Latency-Flags des Decoders nicht mehr.
- **HEVC-Latenzflags** ist jetzt Auto, Ein oder Aus. Auto lässt sie nur auf neueren Amlogic-Boxen weg, Ein verwendet sie auch dort, und Aus lässt sie überall weg, wie der bisherige Schalter.
- Das Debug-Overlay zeigt den Bildtakt: mittleres Intervall, Jitter, Ruckler, wie viele VSyncs jedes Bild gedauert hat und eine Zeitleiste der letzten Bilder.
- Ein Bild, das zu groß für den Decoder-Puffer ist, startet den Decoder mit einem größeren Puffer neu, statt den Stream zu beenden.
- Ein Controller-Gyroskop, das nur Nullen meldet, schaltet Spieler 1 auf die Bewegungssensoren des Geräts um.
