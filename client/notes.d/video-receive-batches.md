## en
- Linux and Android: video packets that arrive together are received in one system call, up to 32 at a time. On our Linux test PC this halves the receive cost per packet when packets queue up, and while a 1000-packet frame streams in, the median packet is handed on 4.5 µs after it was sent instead of 90 µs.
