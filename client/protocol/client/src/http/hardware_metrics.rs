//! The host's CPU and GPU readings (`/api/hardware-metrics`), read as NvHTTP.getHardwareMetrics
//! and HardwareMetrics.java do. Gate on `Feature::HardwareMetrics`; the poll belongs on a worker
//! thread, never the frame path.
//!
//! Optional host APIs share [`HostClient::pinned_get`]: pinned HTTPS to a port already known
//! from serverinfo, so a poll never discovers the port over plain HTTP or reaches an unpinned
//! host.

use super::transport::{self, Request, Timeouts};
use super::{
    Error, Feature, HostApiError, HostCapabilities, HostClient, READ_TIMEOUT, SHORT_CONNECT_TIMEOUT,
};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// The largest reply read.
pub const MAX_HARDWARE_METRICS_BYTES: usize = 256 * 1024;
/// The whole request, connect included, as Android's call timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GPUS: usize = 64;
const MAX_GPU_NAME_UTF16: usize = 512;

/// The CPU readings this client shows; other keys are ignored.
pub const CPU_METRICS: [&str; 1] = ["cpu_usage_percent"];
/// The GPU readings this client shows; other keys are ignored.
pub const GPU_METRICS: [&str; 13] = [
    "gpu_usage_percent",
    "gpu_clock_mhz",
    "gpu_memory_clock_mhz",
    "gpu_temperature_c",
    "gpu_hotspot_temperature_c",
    "gpu_memory_temperature_c",
    "gpu_voltage_mv",
    "gpu_fan_rpm",
    "gpu_fan_percent",
    "gpu_power_w",
    "gpu_board_power_w",
    "gpu_vram_used_mb",
    "gpu_vram_usage_percent",
];

/// Readings the host sent, in [`CPU_METRICS`] or [`GPU_METRICS`] order. A reading the host left
/// out, or sent as null, negative or not a number, is absent; zero stays zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readings(Vec<(&'static str, f64)>);

impl Readings {
    pub fn get(&self, key: &str) -> Option<f64> {
        self.0.iter().find(|(k, _)| *k == key).map(|&(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, f64)> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GpuMetrics {
    pub id: i32,
    pub name: String,
    pub metrics: Readings,
}

/// One sample.
#[derive(Clone, Debug, PartialEq)]
pub struct HardwareMetrics {
    pub sampled_at_unix_ms: i64,
    /// How often the host samples, 500 to 10000 ms.
    pub interval_ms: u32,
    pub cpu: Readings,
    pub gpus: Vec<GpuMetrics>,
}

fn invalid(what: &str) -> HostApiError {
    HostApiError::Host(Error::Malformed(format!("Invalid hardware metrics {what}")))
}

/// An integral JSON number in `[min, max]`; a fraction, a string or a missing key is invalid.
fn integer(
    object: &Map<String, Value>,
    key: &str,
    min: i64,
    max: i64,
) -> Result<i64, HostApiError> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| invalid(key))
}

fn object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, HostApiError> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(key))
}

fn readings(object: &Map<String, Value>, supported: &[&'static str]) -> Readings {
    Readings(
        supported
            .iter()
            .filter_map(|&key| {
                let value = object.get(key)?.as_f64()?;
                (value.is_finite() && value >= 0.0).then_some((key, value))
            })
            .collect(),
    )
}

impl HardwareMetrics {
    /// A reply body. A schema version other than 1 is [`HostApiError::UnsupportedVersion`].
    pub fn parse(json: &str) -> Result<Self, HostApiError> {
        if json.len() > MAX_HARDWARE_METRICS_BYTES {
            return Err(invalid("response size"));
        }
        let root: Value = serde_json::from_str(json).map_err(|_| invalid("response"))?;
        let root = root.as_object().ok_or_else(|| invalid("response"))?;
        if integer(root, "version", 0, i32::MAX.into())? != 1 {
            return Err(HostApiError::UnsupportedVersion);
        }
        let sampled_at_unix_ms = integer(root, "sampled_at_unix_ms", 0, i64::MAX)?;
        let interval_ms = integer(root, "interval_ms", 500, 10_000)? as u32;
        let cpu = readings(object(root, "cpu")?, &CPU_METRICS);
        let entries = root
            .get("gpus")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("gpus"))?;
        if entries.len() > MAX_GPUS {
            return Err(invalid("GPU count"));
        }
        let gpus = entries
            .iter()
            .map(|entry| {
                let entry = entry.as_object().ok_or_else(|| invalid("GPU"))?;
                let id = integer(entry, "id", i32::MIN.into(), i32::MAX.into())? as i32;
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| name.encode_utf16().count() <= MAX_GPU_NAME_UTF16)
                    .ok_or_else(|| invalid("GPU name"))?;
                Ok(GpuMetrics {
                    id,
                    name: name.to_owned(),
                    metrics: readings(object(entry, "metrics")?, &GPU_METRICS),
                })
            })
            .collect::<Result<_, HostApiError>>()?;
        Ok(Self {
            sampled_at_unix_ms,
            interval_ms,
            cpu,
            gpus,
        })
    }
}

/// `application/json` with any parameters.
pub(super) fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|mime| {
        mime.split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("application/json")
    })
}

/// Fails the request from its headers, before the body is read: a status other than 2xx
/// (redirects are not followed), and with `json` any other content type.
pub(super) fn check_head(
    path: &str,
    status: u16,
    headers: &[(String, String)],
    json: bool,
) -> Result<(), Error> {
    match status {
        200..=299 => {}
        404 => return Err(Error::NotFound(format!("/{path}"))),
        code => {
            return Err(Error::Status {
                code: i32::from(code),
                message: format!("the host refused /{path}"),
            })
        }
    }
    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("Content-Type"))
        .map(|(_, value)| value.as_str());
    if json && !is_json(content_type) {
        return Err(Error::Malformed(format!("/{path} is not JSON")));
    }
    Ok(())
}

impl HostClient {
    /// The pinned TLS setup and the HTTPS port from serverinfo, without asking the host.
    pub(super) fn pinned(&self) -> Result<(u16, Arc<rustls::ClientConfig>), Error> {
        let tls = self.tls()?;
        match self.state.lock().unwrap().https_port {
            0 => Err(Error::InvalidArgument(
                "The host's HTTPS port is not known yet; read serverinfo first".into(),
            )),
            port => Ok((port, tls)),
        }
    }

    /// One GET over pinned HTTPS to the HTTPS port from serverinfo, ended after `deadline`.
    pub(super) fn pinned_get(
        &self,
        path: &str,
        limit: usize,
        deadline: Duration,
        json: bool,
    ) -> Result<transport::Response, Error> {
        let (port, tls) = self.pinned()?;
        let target = self.url_target(path, None);
        transport::probe(
            &Request {
                host: &self.host,
                port,
                target: &target,
                tls: Some(tls),
                timeouts: Timeouts {
                    connect: SHORT_CONNECT_TIMEOUT.min(deadline),
                    read: Some(READ_TIMEOUT),
                },
                body_limit: limit,
            },
            &self.cancel,
            deadline,
            &mut |status, headers, _| check_head(path, status, headers, json),
        )
    }

    /// The host's CPU and GPU readings. `capabilities` comes from a paired serverinfo; without
    /// `Feature::HardwareMetrics` nothing is sent and the answer is
    /// [`HostApiError::Unavailable`].
    pub fn hardware_metrics(
        &self,
        capabilities: &HostCapabilities,
    ) -> Result<HardwareMetrics, HostApiError> {
        HostApiError::require(capabilities, Feature::HardwareMetrics)?;
        let response = self.pinned_get(
            "api/hardware-metrics",
            MAX_HARDWARE_METRICS_BYTES,
            CALL_TIMEOUT,
            true,
        )?;
        let body = std::str::from_utf8(&response.body).map_err(|_| invalid("response"))?;
        HardwareMetrics::parse(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::serverinfo::tests::RUBYLIGHT;
    use crate::http::{Availability, Reason, ServerInfo};

    /// The fixtures of Android's HardwareMetricsTest.
    const EMPTY: &str =
        r#"{"version":1,"sampled_at_unix_ms":1791712800000,"interval_ms":1000,"cpu":{},"gpus":[]}"#;

    fn malformed(result: Result<HardwareMetrics, HostApiError>) -> bool {
        matches!(result, Err(HostApiError::Host(Error::Malformed(_))))
    }

    #[test]
    fn optional_readings_preserve_zero_and_do_not_invent_missing_values() {
        let sample = HardwareMetrics::parse(
            &EMPTY
                .replace(r#""cpu":{}"#, r#""cpu":{"cpu_usage_percent":41.6,"future_cpu_metric":99}"#)
                .replace(
                    r#""gpus":[]"#,
                    r#""gpus":[{"id":-12,"name":"AMD Radeon","metrics":{"gpu_usage_percent":0,"gpu_fan_rpm":0,"gpu_temperature_c":61.5,"gpu_power_w":null,"gpu_clock_mhz":"1234","gpu_voltage_mv":-1,"gpu_memory_clock_mhz":"NaN","future_gpu_metric":123}}]"#,
                ),
        )
        .unwrap();
        assert_eq!(sample.sampled_at_unix_ms, 1_791_712_800_000);
        assert_eq!(sample.interval_ms, 1000);
        assert_eq!(
            sample.cpu.iter().collect::<Vec<_>>(),
            [("cpu_usage_percent", 41.6)]
        );
        let gpu = &sample.gpus[0];
        assert_eq!((gpu.id, gpu.name.as_str()), (-12, "AMD Radeon"));
        assert_eq!(
            gpu.metrics.iter().collect::<Vec<_>>(),
            [
                ("gpu_usage_percent", 0.0),
                ("gpu_temperature_c", 61.5),
                ("gpu_fan_rpm", 0.0)
            ]
        );
        assert_eq!(gpu.metrics.get("gpu_power_w"), None);
        let empty = HardwareMetrics::parse(EMPTY).unwrap();
        assert!(empty.cpu.is_empty() && empty.gpus.is_empty());
    }

    #[test]
    fn all_published_gpu_readings_and_multiple_devices_are_preserved() {
        let readings = GPU_METRICS.map(|key| format!(r#""{key}":12.5"#)).join(",");
        let sample = HardwareMetrics::parse(&EMPTY.replace(
            r#""gpus":[]"#,
            &format!(
                r#""gpus":[{{"id":2147483647,"name":"AMD 1","metrics":{{{readings}}}}},{{"id":-2147483648,"name":"","metrics":{{}}}}]"#
            ),
        ))
        .unwrap();
        assert_eq!(sample.gpus.len(), 2);
        assert_eq!(sample.gpus[0].metrics.len(), GPU_METRICS.len());
        for key in GPU_METRICS {
            assert_eq!(sample.gpus[0].metrics.get(key), Some(12.5), "{key}");
        }
        assert_eq!(
            (sample.gpus[1].id, sample.gpus[1].metrics.is_empty()),
            (i32::MIN, true)
        );
    }

    #[test]
    fn unknown_schemas_and_invalid_required_fields_cannot_become_samples() {
        assert!(matches!(
            HardwareMetrics::parse(&EMPTY.replace(r#""version":1"#, r#""version":2"#)),
            Err(HostApiError::UnsupportedVersion)
        ));
        for json in [
            "not json".to_owned(),
            "{}".to_owned(),
            "[]".to_owned(),
            EMPTY.replace(r#""version":1"#, r#""version":"1""#),
            EMPTY.replace("1791712800000", "-1"),
            EMPTY.replace("1791712800000", "9223372036854775808"),
            EMPTY.replace("1000", "499"),
            EMPTY.replace("1000", "10001"),
            EMPTY.replace("1000", "1000.5"),
            EMPTY.replace(r#""cpu":{}"#, r#""cpu":[]"#),
            EMPTY.replace(r#""cpu":{}"#, r#""cpu":{"cpu_usage_percent":1e999}"#),
            EMPTY.replace(r#""gpus":[]"#, r#""gpus":{}"#),
            EMPTY.replace(r#""gpus":[]"#, r#""gpus":[{"id":1,"name":1,"metrics":{}}]"#),
            EMPTY.replace(
                r#""gpus":[]"#,
                r#""gpus":[{"id":2147483648,"name":"","metrics":{}}]"#,
            ),
        ] {
            assert!(malformed(HardwareMetrics::parse(&json)), "{json}");
        }
    }

    #[test]
    fn response_device_counts_and_names_are_bounded() {
        let gpu = r#"{"id":1,"name":"","metrics":{}}"#;
        let devices = vec![gpu; 64].join(",");
        let with = |gpus: &str| EMPTY.replace(r#""gpus":[]"#, &format!(r#""gpus":[{gpus}]"#));
        assert_eq!(
            HardwareMetrics::parse(&with(&devices)).unwrap().gpus.len(),
            64
        );
        assert!(malformed(HardwareMetrics::parse(&with(&format!(
            "{devices},{gpu}"
        )))));
        let named = |name: &str| with(&gpu.replace(r#""name":"""#, &format!(r#""name":"{name}""#)));
        assert!(HardwareMetrics::parse(&named(&"a".repeat(512))).is_ok());
        assert!(malformed(HardwareMetrics::parse(&named(&"a".repeat(513)))));
        assert!(malformed(HardwareMetrics::parse(
            &" ".repeat(MAX_HARDWARE_METRICS_BYTES + 1)
        )));
    }

    #[test]
    fn only_2xx_json_answers_are_read() {
        let json = [(
            "content-type".to_owned(),
            "Application/JSON; charset=utf-8".to_owned(),
        )];
        assert!(check_head("api/hardware-metrics", 200, &json, true).is_ok());
        let html = [("Content-Type".to_owned(), "text/html".to_owned())];
        assert!(matches!(
            check_head("api/hardware-metrics", 200, &html, true),
            Err(Error::Malformed(_))
        ));
        assert!(matches!(
            check_head("api/hardware-metrics", 200, &[], true),
            Err(Error::Malformed(_))
        ));
        assert!(check_head("serverinfo", 200, &html, false).is_ok());
        // Terminal and temporary refusals keep their status; redirects are not followed.
        for code in [302, 403, 409, 503] {
            let error = check_head("api/hardware-metrics", code, &json, true).unwrap_err();
            assert_eq!(error.http_status(), Some(code));
        }
        assert!(matches!(
            check_head("api/hardware-metrics", 404, &json, true),
            Err(Error::NotFound(path)) if path == "/api/hardware-metrics"
        ));
    }

    fn client(pinned: bool, https_port: u16) -> HostClient {
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(https_port);
        if pinned {
            client.pin(Some(client.identity().certificate_der().to_vec()));
        }
        client
    }

    #[test]
    fn a_host_without_the_feature_is_never_asked() {
        // Port 1 on loopback refuses connections: an Io error would mean a request went out.
        let client = client(true, 1);
        let off = ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities;
        assert!(matches!(
            client.hardware_metrics(&off),
            Err(HostApiError::Unavailable {
                feature: Feature::HardwareMetrics,
                availability: Availability::Unsupported(Some(Reason::HardwareMetricsOff)),
            })
        ));
        assert!(matches!(
            client.hardware_metrics(&HostCapabilities::UNKNOWN),
            Err(HostApiError::Unavailable {
                availability: Availability::Unknown,
                ..
            })
        ));
    }

    #[test]
    fn polling_needs_the_pin_and_a_known_port_and_never_discovers_them() {
        let on = ServerInfo::parse(
            &RUBYLIGHT.replace("<HardwareMetricsVersion>0", "<HardwareMetricsVersion>1"),
            47989,
        )
        .unwrap()
        .capabilities;
        assert!(on.has(Feature::HardwareMetrics));
        assert!(matches!(
            client(false, 1).hardware_metrics(&on),
            Err(HostApiError::Host(Error::NotPaired))
        ));
        assert!(matches!(
            client(true, 0).hardware_metrics(&on),
            Err(HostApiError::Host(Error::InvalidArgument(_)))
        ));
        assert!(matches!(
            client(true, 1).hardware_metrics(&on),
            Err(HostApiError::Host(Error::Io(_)))
        ));
    }
}
