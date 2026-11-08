//! Mesa's low-latency VCN decode on AMD (`lowlatencydec` in radeonsi's `AMD_DEBUG` and RADV's
//! `RADV_PERFTEST`), asked for before SDL and the decoders load Mesa, as Nonary's moonlight-qt
//! fork does (`app/main.cpp`, `amddecodepolicy.h`). Drivers without the flag ignore it.
//! `NIGHTJAR_AMD_LOW_LATENCY_DECODE=0` leaves both variables alone. Programs Nightjar starts get
//! the user's own values back (`for_child`).

use std::ffi::OsString;
use std::process::Command;

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

/// The variables as the user started Nightjar with them, once `request` changed them.
#[cfg(target_os = "linux")]
static ORIGINALS: std::sync::OnceLock<[(&str, Option<OsString>); 2]> = std::sync::OnceLock::new();

/// Sets both variables for this process, before SDL_Init and before anything loads Mesa. The
/// environment is not thread-safe: the only thread already running then, the log writer, reads
/// none of it, and the threads that do (SDL's, the decoders', Mesa's) start after this.
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
    let _ = ORIGINALS.set(["AMD_DEBUG", "RADV_PERFTEST"].map(|name| (name, std::env::var_os(name))));
    std::env::set_var("AMD_DEBUG", amd);
    std::env::set_var("RADV_PERFTEST", radv);
}

/// `command` with the user's own `AMD_DEBUG` and `RADV_PERFTEST` (unset when they had none), so
/// a file manager, helper or font tool started from Nightjar does not run with its decode flags.
pub fn for_child(command: &mut Command) -> &mut Command {
    #[cfg(target_os = "linux")]
    if let Some(originals) = ORIGINALS.get() {
        restore(command, originals);
    }
    command
}

fn restore<'a>(command: &'a mut Command, originals: &[(&str, Option<OsString>)]) -> &'a mut Command {
    for (name, value) in originals {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
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

    /// A child gets the user's values back: their own flags, or none at all.
    #[test]
    fn children_get_the_users_own_variables() {
        let mut command = Command::new("true");
        restore(&mut command, &[("AMD_DEBUG", Some("nodcc".into())), ("RADV_PERFTEST", None)]);
        let envs = command.get_envs().map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned))).collect::<Vec<_>>();
        assert_eq!(envs, [(OsString::from("AMD_DEBUG"), Some(OsString::from("nodcc"))), (OsString::from("RADV_PERFTEST"), None)]);
        // Before `request` (and off Linux) nothing changes.
        let mut untouched = Command::new("true");
        assert_eq!(for_child(&mut untouched).get_envs().count(), 0);
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
