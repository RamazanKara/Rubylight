# Hardware metrics for clients

[Documentation](README.md) · [Control messages](control-messages.md) · [HTTP API](api.md)

Rubylight can send host CPU usage and AMD GPU readings to a streaming client for its own overlay. This page describes version 1 of the client protocol. The client draws the overlay; the host's video is unchanged. Use capability discovery rather than the host's version string to detect support.

## Enable on the host

In **All settings → General → Hardware metrics**, enable **Send hardware metrics to client**. The setting is off by default. The host collects readings only when an eligible client requests them during its video stream; enabling the setting alone does not start background polling.

| Configuration key | Default | Meaning |
| --- | --- | --- |
| `hardware_metrics` | `false` | Allow streaming clients to request hardware readings. |
| `hardware_metrics_interval` | `1000` | Minimum time between fresh samples, in milliseconds; configurable from `500` to `10000`. |
| `hardware_metrics_selection` | `[]` | All supported readings when empty, or a list of metric identifiers from the table below. |

These are host-wide settings. GPU readings require an AMD driver with ADLX support; CPU usage comes from Windows and can be available without ADLX. CPU temperature and CPU clock readings are not provided by version 1.

## Discover and authenticate

Use the client's existing GameStream HTTPS connection, presenting the certificate and private key used to pair with this host and verifying the host identity retained during pairing. Read `/serverinfo` over that authenticated connection:

```xml
<HardwareMetricsVersion>1</HardwareMetricsVersion>
```

`1` means the host has enabled this protocol for an authenticated device with **View streams** or **Launch applications** permission. `0`, a missing field, or an unsupported version means the client should not request metrics. Plain HTTP discovery reports `0`. Advertising version 1 does not guarantee an AMD GPU or any particular reading is available, and does not require a stream to have started yet.

Request samples with:

```http
GET /api/hardware-metrics HTTP/1.1
Host: HOST:HTTPS_PORT
Accept: application/json
```

Use `HttpsPort` from `/serverinfo` (normally `47984`, or the configured base port minus 5). This endpoint belongs to the streaming HTTPS service. The administration console's port, Basic authentication, session cookies and API tokens do not authenticate it.

The same paired device must have an active video stream when it requests a sample. Normal streams and Remote Monitor streams qualify; an input-only session, a pending or stopping stream, or another device's stream does not. The request has no body or required query parameters, and is itself the client's explicit request for collection. There is no telemetry control-stream message or subscription.

## Success response

HTTP `200`, `Content-Type: application/json`, `Cache-Control: no-store`. For example, a host with a subset of readings available might return:

```json
{
  "version": 1,
  "sampled_at_unix_ms": 1791712800000,
  "interval_ms": 1000,
  "cpu": {
    "cpu_usage_percent": 41.6
  },
  "gpus": [
    {
      "id": 11520,
      "name": "AMD Radeon RX 7900 XT",
      "metrics": {
        "gpu_temperature_c": 47.0,
        "gpu_hotspot_temperature_c": 51.0,
        "gpu_board_power_w": 28.0,
        "gpu_fan_rpm": 0.0,
        "gpu_vram_used_mb": 2679.0,
        "gpu_vram_usage_percent": 13.09
      }
    }
  ]
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | Integer | Response schema version, currently `1`. |
| `sampled_at_unix_ms` | Unsigned 64-bit integer | Host wall-clock time at sample completion, in milliseconds since the Unix epoch. Cached responses retain this timestamp. |
| `interval_ms` | Integer | Current host sampling interval in milliseconds. |
| `cpu` | Object of numbers | Available, selected CPU readings. May be `{}`. |
| `gpus` | Array | AMD GPUs for which a sample could be obtained. May be `[]`. |
| `gpus[].id` | Signed 32-bit integer | ADLX GPU identifier; not a display index or an indication that this GPU encodes the stream. Do not rely on array order or persistence across host restarts. |
| `gpus[].name` | String | Driver-provided GPU name; may be empty. |
| `gpus[].metrics` | Object of numbers | Available, selected readings for this GPU. May be `{}`. |

All reading values are JSON numbers, including readings expressed in whole units. Treat each metric as optional: unselected, unsupported, failed or invalid readings are omitted. An absent reading means unavailable, not zero; a present zero is valid, for example a stopped fan. CPU usage may be absent from the first sample until a usable time delta exists. GPU entries can disappear when a driver sample fails. Clear or mark those readings unavailable instead of carrying forward stale values.

Ignore unknown fields and metric names within a supported schema version. If the response version is unsupported, stop consuming it. The GPU list is hardware-wide; clients must not assume it identifies the capture or encoding GPU.

## Metric identifiers and units

Only `cpu_usage_percent` appears in `cpu`; every `gpu_` metric appears in each GPU's `metrics` object. Availability varies by GPU and driver.

| Identifier | Unit | Reading |
| --- | --- | --- |
| `cpu_usage_percent` | Percent | Windows CPU busy time over the interval between CPU samples. |
| `gpu_usage_percent` | Percent | GPU utilization reported by ADLX. |
| `gpu_clock_mhz` | MHz | GPU clock. |
| `gpu_memory_clock_mhz` | MHz | GPU memory clock as reported by the driver. |
| `gpu_temperature_c` | °C | GPU temperature. |
| `gpu_hotspot_temperature_c` | °C | GPU hotspot temperature. |
| `gpu_memory_temperature_c` | °C | GPU memory temperature. |
| `gpu_voltage_mv` | mV | GPU voltage. |
| `gpu_fan_rpm` | RPM | Fan speed. |
| `gpu_fan_percent` | Percent | Fan speed percentage reported by ADLX. |
| `gpu_power_w` | W | GPU power reported by ADLX. |
| `gpu_board_power_w` | W | Total board power reported by ADLX; separate from GPU power. |
| `gpu_vram_used_mb` | MB | VRAM used, in the driver's reported MB units. |
| `gpu_vram_usage_percent` | Percent | VRAM used divided by the GPU's reported total VRAM, multiplied by 100. |

## Polling and stream lifetime

Once discovery succeeds and the client's video stream is active, request the first sample. Keep one request in flight and wait at least `interval_ms` from the latest successful response before polling again; the default is one second. Re-read the interval on each response because the host can change it while streaming.

The host samples on demand and shares a cached sample across eligible clients within the configured interval. Polling faster returns the same readings and timestamp. `Cache-Control: no-store` prevents HTTP caching; it does not disable this host-side sampling cache. Use the timestamp to identify repeated samples, and local elapsed time to mark data stale if polling fails; host and client clocks may differ.

Stop polling when the overlay is disabled or the stream ends. Start discovery again on reconnect. The host stops collecting when requests stop, and rejects requests once the stream ends or the setting is disabled. Metrics requests and failures should run independently of video playback.

## Errors

The endpoint returns an HTTP error status with `Content-Type: application/json`, `Cache-Control: no-store` and an `error` string, for example:

```json
{"error": "An active video stream is required"}
```

| HTTP status | Current error strings | Client action |
| --- | --- | --- |
| `403` | `An authenticated viewer is required`; `Hardware metrics are disabled` | Stop polling. Check pairing, device permissions and the host setting before rediscovering support. |
| `409` | `An active video stream is required` | Stop polling until this device has an active video stream. |
| `503` | `Hardware metrics are unavailable`; `Hardware metrics are busy`; `Hardware metrics timed out`; `Hardware metric selection changed` | Mark readings unavailable and retry with backoff while the stream remains active. |

Use the HTTP status for control flow and the text for diagnostics. The host times out a queued/sample response after two seconds. A missing AMD driver or unsupported readings can instead produce a successful response with empty or partial metrics. A `404` means the route is unavailable, including on plain HTTP; stop polling and check discovery and the HTTPS port.

The implementation lives in the [HTTPS handler and capability advertisement](../rust/host/src/nvhttp.rs), [sampling and session checks](../rust/host/src/telemetry.rs), and [Windows metric collector](../rust/windows/src/telemetry.rs).
