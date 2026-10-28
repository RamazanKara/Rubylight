# Webhooks

[Documentation](README.md)

The host can tell other services when devices pair, apps start and stop, and streams begin and end. Each event is an HTTP POST with a JSON body, signed with a secret only the host and the receiver know.

## Set up a hook

Open **Settings → Network → Webhooks** in the web console (or go to `/webhooks`) and add a hook: its URL (`http://` or `https://`) and the events it receives. The console shows the hook's secret once, right after it is created; copy it into the receiver. **New secret** replaces it and shows the new one once; so does moving a hook to another host, since the old secret is the old receiver's. The console refuses URLs on this PC (`localhost`, `127.0.0.0/8`, `::1`) and link-local addresses (`169.254.0.0/16`, `fe80::/10`), whose services may trust whoever reaches them; receivers on your LAN, such as Home Assistant, are fine. **Send test** sends a `webhook.test` event at once and shows what the receiver answered. Each hook lists its last delivery since the host started.

The host keeps the hooks in the `webhooks` setting of its configuration file, with the other host secrets in `ProgramData\Butterpollo`. It never shows the secrets again: the settings API leaves the setting out and refuses to write it, the support bundle redacts it, and the host log names a hook by its origin (`https://example.com`) only, since a path or query may hold a token. At most 16 hooks can be saved.

Scripts can manage hooks through the same API (`GET`/`POST /api/webhooks`, `POST /api/webhooks/delete`, `POST /api/webhooks/test`) with an [API token](api.md).

## Events

| Event | When | `data` |
| --- | --- | --- |
| `pairing.paired` | A device finished pairing (Moonlight PIN or one-time PIN, or a WebRTC browser approved) | `device`, `transport` (`moonlight` or `webrtc`) |
| `pairing.unpaired` | A device was removed, or a WebRTC browser past the limit was dropped | `device`, and `reason: "limit"` for the latter |
| `app.started` | An app started (from a device, the console, or after a deferred sign-in) | `app` (`id`, `name`), `device` (null for the console) |
| `app.stopped` | The running app ended or was closed | `app`, `device`, `reason`, `by` (the device that closed it, or null) |
| `session.started` | A stream session started | `session`, `device`, `app_id`, `role` (`stream`, `remote_monitor`, `input_only`), `transport` |
| `session.ended` | A stream session ended | as `session.started`, plus `failed` |
| `webhook.test` | **Send test** in the console | `{}` |

A `device` is `{"uuid": "...", "name": "..."}`.

## Request format

```http
POST /your/path HTTP/1.1
Content-Type: application/json
X-Rubylight-Event: app.started
X-Rubylight-Delivery: 5b0c3f8e-...
X-Rubylight-Timestamp: 1793145600000
X-Rubylight-Signature: sha256=<hex HMAC-SHA256 of the body with the secret>

{"event":"app.started","delivery":"5b0c3f8e-...","time_unix_ms":1793145600000,
 "host":{"name":"GAMING-PC","unique_id":"..."},"data":{"app":{"id":1,"name":"Desktop"},"device":null}}
```

`X-Rubylight-Timestamp` and `time_unix_ms` are the same: when this try was sent, in Unix milliseconds. Each try of a delivery is stamped and signed anew and keeps its `delivery` id.

## Verify a delivery

1. Compute HMAC-SHA256 of the raw body bytes with the secret (the text the console showed, used as UTF-8 bytes) and compare `sha256=<hex>` with `X-Rubylight-Signature` in constant time.
2. **Replay window:** refuse a delivery whose `time_unix_ms` is more than 5 minutes from your clock. Every try carries its own time, so a delivery that waited in the host's queue still falls inside it; the host drops one still waiting 5 minutes after its event.
3. Remember the `delivery` ids you accepted for those 5 minutes and drop repeats: a retry after a lost answer arrives again with the same id.

```python
import hashlib, hmac, json, time

def verify(secret: str, headers, body: bytes) -> dict:
    expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(expected, headers["X-Rubylight-Signature"]):
        raise ValueError("bad signature")
    event = json.loads(body)
    if abs(time.time() * 1000 - event["time_unix_ms"]) > 5 * 60 * 1000:
        raise ValueError("outside the replay window")
    return event  # then drop it if event["delivery"] was seen in the last 5 minutes
```

## Delivery

Answer with any 2xx status to accept a delivery. The host tries each delivery up to 4 times, waiting 1, 2 and 4 seconds between tries, and gives each try 5 seconds. It retries on no answer, on connection errors and on 5xx, 408 and 429; another 4xx ends the delivery. It does not follow redirects.

Each hook has its own queue of 32 deliveries and sends them in order; a slow or dead receiver delays only its own events, never a stream or another hook. When the queue is full, new events for that hook are dropped, and so are deliveries still queued 5 minutes after their event, which a receiver would refuse as replays. Every dropped or failed delivery is logged as a warning and shown as the hook's last status in the console. Deleting a hook, or moving it to another URL, drops what is still queued for it.
