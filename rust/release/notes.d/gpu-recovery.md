# GPU recovery
- **Compute conversion comes back after a graphics driver reset.** After the graphics card reset during a stream, the stream went on converting colour on the graphics queue for the rest of the session, where a busy game can delay frames. The host now waits briefly for a working compute device and goes back to compute conversion, and a reset no longer counts towards turning compute conversion off for the session.
- On AMD, an encoder that is shut down (at the end of a stream, or when it is rebuilt) first finishes the frames it still had, as AMF expects, for at most 100 ms.
