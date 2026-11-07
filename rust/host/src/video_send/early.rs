//! Whole frames sent block by block (`video_send_early_blocks`).
//!
//! A frame the encoder returned whole goes to an idle paced sender one FEC
//! block at a time, each as soon as it is built, so its first packets leave
//! while the rest still need parity and encryption. 6772d4019 measured it
//! for PyroWave (first block after 0.2 ms instead of 1.8 at 4K); the sender's
//! own thread (031dc57e5) had lost it. Each block is a [`Prepared`] part, as
//! for a streamed frame, and the first carries the whole frame's wire bytes.
use super::*;
use butterpollo_core::network_pacing;

/// Packetize `frame` and enqueue its blocks as they are built. `part` makes
/// a block's [`Prepared`] from its buffers, packet count, layout, place in
/// the frame and, for the first, the frame's wire bytes. Returns the error
/// that dropped the frame, possibly after its first blocks left; fails when
/// the sender stopped.
#[allow(clippy::too_many_arguments)]
pub(super) fn send(
    shared: &Slot,
    packetizer: &mut VideoPacketizer,
    frame: &Encoded,
    timestamp: u32,
    processing: u64,
    layout: &[(usize, usize)],
    peer: SocketAddr,
    part: impl Fn(Vec<PacketBlock>, Vec<usize>, (usize, usize), Part, Option<usize>) -> Prepared,
) -> Result<Option<anyhow::Error>> {
    let packets: usize = layout.iter().map(|(data, parity)| data + parity).sum();
    let frame_wire = packets
        * (datagram(packetizer) + network_pacing::overhead(peer.ip().to_canonical().is_ipv6()));
    let mut next = match packetizer.conventional_blocks(
        &frame.bytes,
        frame.idr,
        frame.after_invalidation,
        timestamp,
        processing,
    ) {
        Ok((_, next)) => next,
        Err(error) => return Ok(Some(error)),
    };
    for (index, &block) in layout.iter().enumerate() {
        let mut parts = shared
            .state
            .lock()
            .unwrap()
            .recycled
            .pop()
            .unwrap_or_default();
        parts.truncate(1);
        if parts.is_empty() {
            parts.push(Vec::new());
        }
        let count = match next(&mut parts[0]) {
            Ok(count) if count == block.0 + block.1 => count,
            built => {
                shared.state.lock().unwrap().recycle(parts);
                return Ok(Some(built.err().unwrap_or_else(|| {
                    anyhow::anyhow!("FEC block {index} does not match the frame's block layout")
                })));
            }
        };
        let place = Part {
            first: index == 0,
            last: index + 1 == layout.len(),
        };
        shared.enqueue(part(
            parts,
            vec![count],
            block,
            place,
            place.first.then_some(frame_wire),
        ))?;
    }
    Ok(None)
}

/// The size of every datagram of a conventional frame: its FEC block and
/// any encryption envelope.
pub(super) fn datagram(packetizer: &VideoPacketizer) -> usize {
    packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 }
}
