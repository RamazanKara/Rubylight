#include "Limelight-internal.h"
#include "pyrowave_protocol.h"
#include "fec_report.h"

void finishPyroWaveFrame(void);

static void notifyOrdinaryFrameLost(unsigned int frame, bool speculative) {
    if (!PyroWaveRecordsEnabled) notifyFrameLost(frame, speculative);
}

// Every block that completed or was given up goes to the FEC loss report. A completed block
// counts only the holes before its last arrival: parity still in flight is not missing.
static void finishFecBlock(PRTP_VIDEO_QUEUE queue, bool unrecovered) {
    uint32_t total = queue->bufferDataPackets + queue->bufferParityPackets;
    uint32_t arrived = queue->receivedDataPackets + queue->receivedParityPackets;
    uint32_t missing;
    if (unrecovered) {
        missing = total > arrived ? total - arrived : 0;
    } else {
        missing = queue->missingPackets < total ? queue->missingPackets : total;
    }
    rlFecBlockFinished(queue->bufferDataPackets, queue->bufferParityPackets, missing, unrecovered);
}

#define RL_FEC_BLOCK_FINISHED finishFecBlock
#define notifyFrameLost notifyOrdinaryFrameLost
#define RtpvAddPacket ordinaryRtpvAddPacket
#include "moonlight-common-c/src/RtpVideoQueue.c"
#undef RtpvAddPacket
#undef notifyFrameLost
#undef RL_FEC_BLOCK_FINISHED

int RtpvAddPacket(PRTP_VIDEO_QUEUE queue, PRTP_PACKET packet, int length, PRTPV_QUEUE_ENTRY entry) {
    if (!PyroWaveRecordsEnabled) return ordinaryRtpvAddPacket(queue, packet, length, entry);
    int offset = sizeof(*packet) + ((packet->header & FLAG_EXTENSION) ? 4 : 0);
    if (length < offset + (int)sizeof(NV_VIDEO_PACKET)) return RTPF_RET_REJECTED;
    PNV_VIDEO_PACKET video = (PNV_VIDEO_PACKET)((char*)packet + offset);
    uint32_t frame = LE32(video->frameIndex);
    uint8_t block = (video->multiFecBlocks >> 4) & 3;
    if (isBefore32(frame, queue->currentFrameNumber) ||
            (frame == queue->currentFrameNumber && block < queue->multiFecCurrentBlockNumber)) {
        return RTPF_RET_REJECTED;
    }
    if (frame != queue->currentFrameNumber || block != queue->multiFecCurrentBlockNumber) {
        if (queue->pendingFecBlockList.count) {
            reportFinalFrameFecStatus(queue);
            finishFecBlock(queue, true);
            // The core's sorter also handles holes; it discards parity but keeps every data packet.
            stageCompleteFecBlock(queue);
        }
        if (frame != queue->currentFrameNumber) {
            submitCompletedFrame(queue);
            finishPyroWaveFrame();
        }
        queue->currentFrameNumber = frame;
        queue->multiFecCurrentBlockNumber = block;
    }
    int result = ordinaryRtpvAddPacket(queue, packet, length, entry);
    if (queue->currentFrameNumber != frame) finishPyroWaveFrame();
    return result;
}
