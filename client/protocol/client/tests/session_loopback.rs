//! A whole session against an in-process host on loopback: the Rubylight host's RTSP
//! helpers answer the handshake, a rusty_enet host plays the control stream with the host's
//! own sealing and answers roaming, and UDP sockets answer the video and audio pings with
//! packetized frames and Opus packets and take the microphone. Like the Rubylight host, it
//! takes media pings only from the session's client address, moves the session only on a
//! CONFIRM of its pending challenge, and forgets the old path's media peers when it does.

#[path = "session_loopback/host.rs"]
mod host;
#[path = "session_loopback/media.rs"]
mod media;
#[path = "session_loopback/recorder.rs"]
mod recorder;
#[path = "session_loopback/roaming.rs"]
mod roaming;
#[path = "session_loopback/streaming.rs"]
mod streaming;
