## en
- A decoder error late in a long stream is recovered again: the ten recoveries a stream gets now refill after 600 good frames and 10 seconds, instead of running out after ten errors hours apart.
- A decoder that stops taking input is reset once before the stream ends, and the stream still ends with a reason if that reset hangs too.
- Desktop: with "PC's frame times", the hardware decoder gets enough surfaces for every picture held after a network hiccup, so its next frames no longer fail at high frame rates.
