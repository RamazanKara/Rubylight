## en
- A stream faster than your screen no longer turns the overlay's status dot amber; the overlay says the screen's refresh rate instead ("60 Hz screen"). Frames missing below the screen's rate still turn it amber or red.
- Statistics ends the stream line with the received frame rate, and the host's present-to-send line is now called "Host incl. send".
- Debug: "On screen" is now "Presented", estimated display drops show as a share of a two-second window instead of a total for the whole stream (five seconds behind, since a drop is only known once the screen's report fails to come), and judder says "not measured" when the stream is faster than the screen. End-to-end latency lines say "(end to end)".
