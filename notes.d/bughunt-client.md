## en
- When the PC ended the connection while a stream was still starting, automatic recovery could end the stream it had just recovered. Each attempt now reports only its own end.
- If the microphone cannot open its network socket, the log now says why instead of the microphone staying off without a word.
- File downloads no longer overwrite a file that is saved under the same name while they run, or another download's unfinished `.part` file. Such a download now stops with an error.
- A download that sends more than the size the PC listed now stops with an error instead of writing on.
- If a failed download's unfinished file cannot be deleted, the error now names it.

## de
- Wenn der PC die Verbindung beendete, während ein Stream noch startete, konnte die automatische Wiederherstellung den gerade wiederhergestellten Stream beenden. Jeder Versuch meldet jetzt nur noch sein eigenes Ende.
- Kann das Mikrofon seinen Netzwerk-Socket nicht öffnen, steht der Grund jetzt im Log, statt dass das Mikrofon ohne Hinweis aus bleibt.
- Downloads überschreiben keine Datei mehr, die während des Downloads unter demselben Namen gespeichert wird, und auch keine unfertige `.part`-Datei eines anderen Downloads. Ein solcher Download bricht jetzt mit einem Fehler ab.
- Ein Download, der mehr sendet als die vom PC angegebene Größe, bricht jetzt mit einem Fehler ab, statt weiterzuschreiben.
- Lässt sich die unfertige Datei eines fehlgeschlagenen Downloads nicht löschen, nennt der Fehler sie jetzt.
