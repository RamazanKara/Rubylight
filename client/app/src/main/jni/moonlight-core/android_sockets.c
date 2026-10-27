// Keep the upstream submodule intact while replacing the affected socket helpers.
#define sendMtuSafe upstreamSendMtuSafe
#define isInSubnetV6 upstreamIsInSubnetV6
#define isPrivateNetworkAddress upstreamIsPrivateNetworkAddress
#include "moonlight-common-c/src/PlatformSockets.c"
#undef sendMtuSafe
#undef isInSubnetV6
#undef isPrivateNetworkAddress
#include "android_sockets.h"
