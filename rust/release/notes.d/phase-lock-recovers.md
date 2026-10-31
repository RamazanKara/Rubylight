# Phase lock recovers
- **Streams no longer stay a few milliseconds behind after the PC was busy.** When the PC was busy for a while (a build, a game loading), the phase lock that times frames to arrive just before the client's screen refreshes could lose that refresh and never find it again: until the stream restarted, frames arrived anywhere in the refresh, about 7 ms later on average and with some judder, and the PC sent slightly more frames than the screen shows. The host now notices when it has lost the refresh and finds it again within a few seconds.
  Tested: TBD
