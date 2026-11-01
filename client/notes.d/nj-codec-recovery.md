## en
- A decoder error late in a long stream is recovered again: the ten recoveries a stream gets now refill after 600 good frames, instead of running out after ten errors hours apart.
- A decoder that stops taking input is reset once before the stream ends, and the stream still ends with a reason if that reset hangs too.
