#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "android_microphone_packet.h"

// What happened to the microphone this session. MoonBridge.MIC_STATE_* mirrors these values.
#define MIC_STATE_OFF 0          // the user has not turned the microphone on
#define MIC_STATE_NOT_OFFERED 1  // the host's DESCRIBE did not offer one
#define MIC_STATE_UNAVAILABLE 2  // offered, but it could not be set up (see the log)
#define MIC_STATE_READY 3        // set up and encrypted; packets can be sent

// Set by startConnection before LiStartConnection: the user wants the microphone sent.
extern bool AndroidMicRequested;
// Port from the host's SETUP streamid=mic reply; 0 when the microphone is not set up.
extern uint16_t MicPortNumber;

void setMicrophoneState(int state);
// Opens the socket and encoder once the session is up; returns 0 or an error.
int initializeMicrophoneStream(void);
// Closes them and logs what was sent. Safe to call at any time and more than once.
void destroyMicrophoneStream(void);
