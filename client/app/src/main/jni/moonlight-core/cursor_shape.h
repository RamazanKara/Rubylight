#pragma once
#include <stdbool.h>
#include <stdint.h>

// Host cursor shapes (docs/protocol/cursor-shape.md). The client asks with 0x5540 only when the
// host named it in a=x-rl-control; the host then sends 0x5541 shape messages. All fields are LE.
#define RL_CURSOR_SHAPE_REQUEST_TYPE 0x5540
#define RL_CURSOR_SHAPE_MESSAGE_TYPE 0x5541
#define RL_CURSOR_SHAPE_VERSION 1
#define RL_CURSOR_SHAPE_REQUEST_BYTES 4
#define RL_CURSOR_SHAPE_MAX_EDGE 96
#define RL_CURSOR_SHAPE_HEADER_BYTES 8
#define RL_CURSOR_SHAPE_IMAGE_HEADER_BYTES 12
#define RL_CURSOR_SHAPE_MAX_PAYLOAD (RL_CURSOR_SHAPE_HEADER_BYTES + RL_CURSOR_SHAPE_IMAGE_HEADER_BYTES + \
                                     RL_CURSOR_SHAPE_MAX_EDGE * RL_CURSOR_SHAPE_MAX_EDGE * 4)

// Per connection in android_control.c: true between an enable request and the next disable
// request or connection start.
void resetCursorShapes(void);
bool cursorShapesRequested(void);

// Android bridge (callbacks.c): copies a borrowed message of RL_CURSOR_SHAPE_HEADER_BYTES to
// RL_CURSOR_SHAPE_MAX_PAYLOAD bytes to Java, which validates and decodes it off this thread.
void BridgeClCursorShape(const uint8_t* payload, int length);
