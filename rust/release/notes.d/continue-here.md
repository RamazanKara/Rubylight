# Continue here

- A paired device with View and Launch can take the running game's stream from another device: `POST /session/continue-here` ends the other device's stream, keeps the game running and tells that device where its stream went for two minutes. Co-op, background, Input-only and Remote Monitor sessions and WebRTC guests are left alone.
- `GET /session` names the device playing the running game for callers with List or View.
- A press that fails changes nothing for good: the game goes back to the first device, still running, and survives close on disconnect while that device can come back.
