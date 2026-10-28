#pragma once

#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

// Single producer (decoder), single consumer (AAudio callback), stereo PCM16.
typedef struct {
    int16_t* samples;
    uint32_t capacity;
    atomic_uint readFrames, writtenFrames, targetFrames, underruns;
    bool primed;
} AudioQueue;

static void audioQueueInit(AudioQueue* queue, int16_t* samples, uint32_t capacity, uint32_t target) {
    queue->samples = samples;
    queue->capacity = capacity;
    queue->primed = false;
    atomic_init(&queue->readFrames, 0);
    atomic_init(&queue->writtenFrames, 0);
    atomic_init(&queue->targetFrames, target < capacity ? target : capacity);
    atomic_init(&queue->underruns, 0);
}

static uint32_t audioQueueWrite(AudioQueue* queue, const int16_t* samples, uint32_t frames) {
    uint32_t written = atomic_load_explicit(&queue->writtenFrames, memory_order_relaxed);
    uint32_t read = atomic_load_explicit(&queue->readFrames, memory_order_acquire);
    uint32_t queued = written - read;
    uint32_t target = atomic_load(&queue->targetFrames);
    uint32_t freeFrames = queued < target ? target - queued : 0;
    uint32_t count = frames < freeFrames ? frames : freeFrames;
    uint32_t offset = written % queue->capacity;
    uint32_t first = count < queue->capacity - offset ? count : queue->capacity - offset;
    memcpy(queue->samples + offset * 2, samples, first * 2 * sizeof(int16_t));
    memcpy(queue->samples, samples + first * 2, (count - first) * 2 * sizeof(int16_t));
    atomic_store_explicit(&queue->writtenFrames, written + count, memory_order_release);
    return count;
}

// Fills samples with frames frames, silence where the queue has none; returns the queued frames played.
static uint32_t audioQueueRead(AudioQueue* queue, int16_t* samples, uint32_t frames) {
    uint32_t read = atomic_load_explicit(&queue->readFrames, memory_order_relaxed);
    uint32_t written = atomic_load_explicit(&queue->writtenFrames, memory_order_acquire);
    uint32_t available = written - read;
    if (!queue->primed) {
        // The target must be a reserve, not just a limit on the writer. Consuming
        // the first packet immediately leaves burst/packet timing differences as
        // recurring gaps, even after the underrun policy increases the target.
        if (available < atomic_load(&queue->targetFrames)) {
            memset(samples, 0, frames * 2 * sizeof(int16_t));
            return 0;
        }
        queue->primed = true;
    }
    uint32_t count = available < frames ? available : frames;
    uint32_t offset = read % queue->capacity;
    uint32_t first = count < queue->capacity - offset ? count : queue->capacity - offset;
    memcpy(samples, queue->samples + offset * 2, first * 2 * sizeof(int16_t));
    memcpy(samples + first * 2, queue->samples, (count - first) * 2 * sizeof(int16_t));
    if (count < frames) {
        memset(samples + count * 2, 0, (frames - count) * 2 * sizeof(int16_t));
        // Count a starvation once, then wait for a usable reserve before resuming.
        // Counting every silent callback would inflate the adaptive buffer size.
        atomic_fetch_add_explicit(&queue->underruns, 1, memory_order_relaxed);
        queue->primed = false;
    }
    atomic_store_explicit(&queue->readFrames, read + count, memory_order_release);
    return count;
}
