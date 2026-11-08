//! Mesa's low-latency VCN decode on AMD (`lowlatencydec` in radeonsi's `AMD_DEBUG` and RADV's
//! `RADV_PERFTEST`), asked for before SDL and the decoders load Mesa, as Nonary's moonlight-qt
//! fork does (`app/main.cpp`, `amddecodepolicy.h`). Drivers without the flag ignore it.
//! `NIGHTJAR_AMD_LOW_LATENCY_DECODE=0` leaves both variables alone.

const FLAG: &str = "lowlatencydec";

/// `flags` with `lowlatencydec` appended, keeping the user's own flags; unchanged when it is
/// already there, so a second call adds nothing.
pub fn with_low_latency_decode(flags: &str) -> String {
    if flags.split(',').any(|flag| flag.trim() == FLAG) {
        return flags.to_owned();
    }
    let mut out = flags.to_owned();
    if !out.is_empty() && !out.ends_with(',') {
        out.push(',');
    }
    out.push_str(FLAG);
    out
}

/// What the variables become: (`AMD_DEBUG`, `RADV_PERFTEST`), or `None` when switched off.
/// radeonsi still reads the older `R600_DEBUG` when `AMD_DEBUG` is unset, so its flags carry over.
pub fn variables(switch: Option<&str>, amd_debug: Option<&str>, r600_debug: Option<&str>, radv_perftest: Option<&str>) -> Option<(String, String)> {
    if switch.is_some_and(|value| value.trim() == "0") {
        return None;
    }
    let amd = amd_debug.or(r600_debug).unwrap_or_default();
    Some((with_low_latency_decode(amd), with_low_latency_decode(radv_perftest.unwrap_or_default())))
}

/// Sets both variables for this process. Call before SDL_Init, while no other thread runs.
#[cfg(target_os = "linux")]
pub fn request() {
    let get = |name: &str| std::env::var(name).ok();
    let Some((amd, radv)) = variables(
        get("NIGHTJAR_AMD_LOW_LATENCY_DECODE").as_deref(),
        get("AMD_DEBUG").as_deref(),
        get("R600_DEBUG").as_deref(),
        get("RADV_PERFTEST").as_deref(),
    ) else {
        eprintln!("nightjar: AMD low-latency decode not asked for (NIGHTJAR_AMD_LOW_LATENCY_DECODE=0)");
        return;
    };
    eprintln!("nightjar: AMD low-latency decode asked for: AMD_DEBUG={amd} RADV_PERFTEST={radv} (Mesa drivers with lowlatencydec use it)");
    std::env::set_var("AMD_DEBUG", amd);
    std::env::set_var("RADV_PERFTEST", radv);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_to_empty_and_keeps_user_flags() {
        assert_eq!(with_low_latency_decode(""), "lowlatencydec");
        assert_eq!(with_low_latency_decode("nodcc"), "nodcc,lowlatencydec");
        assert_eq!(with_low_latency_decode("nodcc,"), "nodcc,lowlatencydec");
        assert_eq!(with_low_latency_decode("video_decode,sam"), "video_decode,sam,lowlatencydec");
    }

    #[test]
    fn is_idempotent() {
        assert_eq!(with_low_latency_decode("lowlatencydec"), "lowlatencydec");
        assert_eq!(with_low_latency_decode("nodcc, lowlatencydec"), "nodcc, lowlatencydec");
        let once = with_low_latency_decode("nodcc");
        assert_eq!(with_low_latency_decode(&once), once);
        // A longer flag that only contains the name is a different flag.
        assert_eq!(with_low_latency_decode("nolowlatencydec"), "nolowlatencydec,lowlatencydec");
    }

    #[test]
    fn variables_follow_the_switch_and_r600_fallback() {
        assert_eq!(variables(None, None, None, None), Some(("lowlatencydec".into(), "lowlatencydec".into())));
        assert_eq!(variables(Some("0"), Some("nodcc"), None, None), None);
        assert_eq!(variables(Some("1"), None, None, None), Some(("lowlatencydec".into(), "lowlatencydec".into())));
        assert_eq!(variables(None, None, Some("nohyperz"), Some("video_decode")), Some(("nohyperz,lowlatencydec".into(), "video_decode,lowlatencydec".into())));
        // AMD_DEBUG wins over R600_DEBUG, as in radeonsi.
        assert_eq!(variables(None, Some("nodcc"), Some("nohyperz"), None).unwrap().0, "nodcc,lowlatencydec");
    }
}
