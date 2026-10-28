## en
- When the PC ended the connection while a stream was still starting, automatic recovery could end the stream it had just recovered. Each attempt now reports only its own end.
- If the microphone cannot open its network socket, the log now says why instead of the microphone staying off without a word.
- File downloads no longer overwrite a file that is saved under the same name while they run, or another download's unfinished `.part` file. Such a download now stops with an error.
- A download that sends more than the size the PC listed now stops with an error instead of writing on.
- If a failed download's unfinished file cannot be deleted, the error now names it.
- A damaged image on the clipboard could crash the clipboard sync. It is now refused with an error.
- A Remote Monitor session, which only watches, no longer sends the microphone to the PC.
- With Controllers off, or in a Remote Monitor session, unplugging an audio device no longer reports the connected controllers to the PC.
- A controller that cannot be opened, or one past the 16 the PC takes, now shows a message instead of doing nothing.
- When video decoding keeps failing, for example after a graphics driver reset, the picture no longer freezes without a word: Nightjar opens the decoder again, and if that does not help, the stream ends with a message. A graphics driver reset or a lost GPU during a stream also ends it with a message.

## de
- Wenn der PC die Verbindung beendete, während ein Stream noch startete, konnte die automatische Wiederherstellung den gerade wiederhergestellten Stream beenden. Jeder Versuch meldet jetzt nur noch sein eigenes Ende.
- Kann das Mikrofon seinen Netzwerk-Socket nicht öffnen, steht der Grund jetzt im Log, statt dass das Mikrofon ohne Hinweis aus bleibt.
- Downloads überschreiben keine Datei mehr, die während des Downloads unter demselben Namen gespeichert wird, und auch keine unfertige `.part`-Datei eines anderen Downloads. Ein solcher Download bricht jetzt mit einem Fehler ab.
- Ein Download, der mehr sendet als die vom PC angegebene Größe, bricht jetzt mit einem Fehler ab, statt weiterzuschreiben.
- Lässt sich die unfertige Datei eines fehlgeschlagenen Downloads nicht löschen, nennt der Fehler sie jetzt.
- Ein beschädigtes Bild in der Zwischenablage konnte die Zwischenablage-Synchronisierung abstürzen lassen. Es wird jetzt mit einem Fehler abgelehnt.
- Eine Remote-Monitor-Sitzung, die nur zuschaut, sendet das Mikrofon nicht mehr an den PC.
- Mit ausgeschalteten Controllern oder in einer Remote-Monitor-Sitzung meldet das Abziehen eines Audiogeräts die verbundenen Controller nicht mehr an den PC.
- Ein Controller, der sich nicht öffnen lässt, oder einer über die 16 hinaus, die der PC annimmt, zeigt jetzt eine Meldung, statt dass nichts passiert.
- Schlägt die Videodekodierung immer wieder fehl, etwa nach einem Zurücksetzen des Grafiktreibers, friert das Bild nicht mehr ohne Hinweis ein: Nightjar öffnet den Decoder neu, und hilft das nicht, endet der Stream mit einer Meldung. Ein Zurücksetzen des Grafiktreibers oder eine verlorene GPU während eines Streams beendet ihn ebenfalls mit einer Meldung.
