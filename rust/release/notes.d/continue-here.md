# Continue here

- A paired device with View and Launch can take the running game's stream from another device: `POST /session/continue-here` ends the other device's stream, keeps the game running and tells that device where its stream went for two minutes. Co-op, background, Input-only and Remote Monitor sessions and WebRTC guests are left alone.
- `GET /session` names the device playing the running game for callers with List or View.
- A press that fails changes nothing for good: the game goes back to the first device, still running, and survives close on disconnect for a minute, so that device can come back.
- A device that does not own the game may join it with `/resume` beside the owner. That `/resume` answers 409 when another device holds a plain WebRTC stream, which a Moonlight stream would end, or when its `appuuid` names another game than the running one; a request without `appuuid`, and the owner's own, resolve to the running game. It never starts a game.
