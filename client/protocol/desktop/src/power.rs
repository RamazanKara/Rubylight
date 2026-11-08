//! Shut down, restart or sleep the PC Nightjar runs on: the way out for a living-room PC run by
//! controller alone, asked from Home's Close Nightjar question. Windows asks InitiateShutdownW
//! (or SetSuspendState) with the shutdown privilege; Linux asks logind over D-Bus. Only what the
//! system says it can do is offered (power capabilities, logind's `Can…`), and both the probe and
//! the request run off the UI thread. What each action asks the system for is worked out on its
//! own, so tests never power anything off.

use std::sync::mpsc::{channel, Receiver};

use crate::strings::en;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    ShutDown,
    Restart,
    Sleep,
}

impl PowerAction {
    /// In the order the Close Nightjar question offers them.
    pub const ALL: [Self; 3] = [Self::ShutDown, Self::Restart, Self::Sleep];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShutDown => en("Shut down"),
            Self::Restart => en("Restart"),
            Self::Sleep => en("Sleep"),
        }
    }

    /// The question that confirms it.
    pub fn question(self) -> &'static str {
        match self {
            Self::ShutDown => en("Shut down this PC?"),
            Self::Restart => en("Restart this PC?"),
            Self::Sleep => en("Put this PC to sleep?"),
        }
    }

    /// The title of the message when the system refused, the reason under it.
    pub fn failed(self) -> &'static str {
        match self {
            Self::ShutDown => en("Could not shut down this PC"),
            Self::Restart => en("Could not restart this PC"),
            Self::Sleep => en("Could not put this PC to sleep"),
        }
    }

    /// logind's Manager method for it.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn login1_method(self) -> &'static str {
        match self {
            Self::ShutDown => "PowerOff",
            Self::Restart => "Reboot",
            Self::Sleep => "Suspend",
        }
    }

    /// InitiateShutdownW's flags, None for sleep (SetSuspendState). Other apps are asked to
    /// close, not forced, as the Start menu does.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn shutdown_flags(self) -> Option<u32> {
        // SHUTDOWN_POWEROFF and SHUTDOWN_RESTART.
        match self {
            Self::ShutDown => Some(0x0000_0008),
            Self::Restart => Some(0x0000_0004),
            Self::Sleep => None,
        }
    }

    /// logind's Manager method that says whether it may be done.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn login1_can(self) -> &'static str {
        match self {
            Self::ShutDown => "CanPowerOff",
            Self::Restart => "CanReboot",
            Self::Sleep => "CanSuspend",
        }
    }
}

/// An action the system says this session may take. `interactive`: logind wants a password
/// first ("challenge"), so the request lets polkit ask for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub action: PowerAction,
    pub interactive: bool,
}

/// Windows' offers by its power capabilities: shut down and restart always, Sleep only with a
/// sleep state (S1 to S3). A Modern Standby PC (no S3) gets no Sleep: SetSuspendState cannot put
/// it into standby and may hibernate it instead.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_offers(sleep_state: bool) -> Vec<Offer> {
    PowerAction::ALL
        .into_iter()
        .filter(|a| *a != PowerAction::Sleep || sleep_state)
        .map(|action| Offer { action, interactive: false })
        .collect()
}

/// logind's answer to `Can…` for `action`: "yes" offers it, "challenge" offers it behind a
/// password prompt, "no", "na" or no answer leave it out.
#[cfg_attr(windows, allow(dead_code))]
pub fn logind_offer(action: PowerAction, answer: Option<&str>) -> Option<Offer> {
    match answer? {
        "yes" => Some(Offer { action, interactive: false }),
        "challenge" => Some(Offer { action, interactive: true }),
        _ => None,
    }
}

/// What this PC offers, asked off the UI thread; `waker` wakes the UI once it is known.
pub fn probe(waker: crate::hosts::Waker) -> Receiver<Vec<Offer>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let offers = offers();
        eprintln!("nightjar: power offers {:?}", offers.iter().map(|o| o.action).collect::<Vec<_>>());
        let _ = tx.send(offers);
        waker();
    });
    rx
}

/// Asks the system for `offer` off the UI thread (logind may wait on its password prompt, and
/// Windows' sleep returns only on wake); the answer arrives on the receiver.
pub fn request(offer: Offer, waker: crate::hosts::Waker) -> Receiver<Result<(), String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(perform(offer));
        waker();
    });
    rx
}

#[cfg(windows)]
fn offers() -> Vec<Offer> {
    use windows::Win32::System::Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES};
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: a struct for the call to fill in.
    if !unsafe { GetPwrCapabilities(&mut caps) } {
        eprintln!("nightjar: power capabilities unknown: {}", windows::core::Error::from_thread().message());
        return windows_offers(false);
    }
    windows_offers(caps.SystemS1 || caps.SystemS2 || caps.SystemS3)
}

#[cfg(not(windows))]
fn offers() -> Vec<Offer> {
    use zbus::blocking::{connection::Builder, Proxy};
    let manager = (|| -> zbus::Result<Proxy<'static>> {
        let connection = Builder::system()?.method_timeout(std::time::Duration::from_secs(5)).build()?;
        Proxy::new_owned(connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")
    })();
    let manager = match manager {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("nightjar: logind not reachable: {e}");
            return Vec::new();
        }
    };
    PowerAction::ALL
        .into_iter()
        .filter_map(|action| {
            let answer = manager.call::<_, _, String>(action.login1_can(), &()).map_err(|e| eprintln!("nightjar: {}: {e}", action.login1_can())).ok();
            logind_offer(action, answer.as_deref())
        })
        .collect()
}

/// Asks the system for `offer`. Shut down and restart return once they are under way; sleep on
/// Windows returns once the PC wakes again. The error is the system's reason, for a message.
#[cfg(windows)]
fn perform(offer: Offer) -> Result<(), String> {
    let action = offer.action;
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{
        InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FLAGS,
    };
    enable_shutdown_privilege()?;
    match action.shutdown_flags() {
        Some(flags) => {
            let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
            // SAFETY: no machine name (this PC) and no message; plain values otherwise.
            let error = unsafe { InitiateShutdownW(None, None, 0, SHUTDOWN_FLAGS(flags), reason) };
            match error {
                0 => Ok(()),
                code => Err(windows::core::Error::from(windows::core::HRESULT::from_win32(code)).message()),
            }
        }
        // SAFETY: plain values: sleep, not hibernate, not forced, wake events stay on.
        None => match unsafe { SetSuspendState(false, false, false) } {
            true => Ok(()),
            false => Err(windows::core::Error::from_thread().message()),
        },
    }
}

/// The shutdown privilege, which a normal user has but must switch on before asking.
#[cfg(windows)]
fn enable_shutdown_privilege() -> Result<(), String> {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of this process and a handle to fill in.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }.map_err(|e| e.message())?;
    let result = (|| {
        let mut luid = LUID::default();
        // SAFETY: a constant privilege name and a LUID to fill in.
        unsafe { LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid) }.map_err(|e| e.message())?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        // SAFETY: the token opened above for adjusting, and one privilege that lives across the call.
        unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) }.map_err(|e| e.message())?;
        // It succeeds without granting a privilege the account does not have: that is told apart here.
        // SAFETY: reads this thread's last error, set by the call above.
        if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
            return Err(windows::core::Error::from(ERROR_NOT_ALL_ASSIGNED.to_hresult()).message());
        }
        Ok(())
    })();
    // SAFETY: the token opened above, closed once.
    let _ = unsafe { CloseHandle(token) };
    result
}

/// Asks logind. A password prompt only where logind said it needs one ("challenge"), and then
/// with time to type it; otherwise none, so a refusal comes back as the reason.
#[cfg(not(windows))]
fn perform(offer: Offer) -> Result<(), String> {
    use zbus::blocking::{connection::Builder, Proxy};
    let wait = std::time::Duration::from_secs(if offer.interactive { 120 } else { 5 });
    let call = || -> zbus::Result<()> {
        let connection = Builder::system()?.method_timeout(wait).build()?;
        let manager = Proxy::new(&connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")?;
        // The one argument is `interactive`.
        manager.call::<_, _, ()>(offer.action.login1_method(), &(offer.interactive,))
    };
    call().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_asks_the_system_for_its_own_thing() {
        let methods: Vec<&str> = PowerAction::ALL.iter().map(|a| a.login1_method()).collect();
        assert_eq!(methods, ["PowerOff", "Reboot", "Suspend"]);
        // SHUTDOWN_POWEROFF, SHUTDOWN_RESTART, and sleep through SetSuspendState instead.
        let flags: Vec<Option<u32>> = PowerAction::ALL.iter().map(|a| a.shutdown_flags()).collect();
        assert_eq!(flags, [Some(8), Some(4), None]);
        // Never forced: other apps get to close (SHUTDOWN_FORCE_OTHERS, SHUTDOWN_FORCE_SELF).
        assert!(flags.iter().flatten().all(|f| f & 0x1 == 0 && f & 0x2 == 0));
    }

    #[test]
    fn windows_offers_sleep_only_with_a_sleep_state() {
        let actions = |offers: Vec<Offer>| offers.into_iter().map(|o| o.action).collect::<Vec<_>>();
        assert_eq!(actions(windows_offers(true)), PowerAction::ALL);
        // Modern Standby: no S1 to S3, so no Sleep; shut down and restart stay.
        assert_eq!(actions(windows_offers(false)), [PowerAction::ShutDown, PowerAction::Restart]);
        assert!(windows_offers(true).iter().all(|o| !o.interactive));
    }

    #[test]
    fn logind_offers_what_it_allows_and_asks_for_a_password_on_challenge() {
        let methods: Vec<&str> = PowerAction::ALL.iter().map(|a| a.login1_can()).collect();
        assert_eq!(methods, ["CanPowerOff", "CanReboot", "CanSuspend"]);
        let sleep = PowerAction::Sleep;
        assert_eq!(logind_offer(sleep, Some("yes")), Some(Offer { action: sleep, interactive: false }));
        assert_eq!(logind_offer(sleep, Some("challenge")), Some(Offer { action: sleep, interactive: true }));
        for answer in [Some("no"), Some("na"), Some("maybe"), None] {
            assert_eq!(logind_offer(sleep, answer), None, "{answer:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_flags_are_windows_own() {
        use windows::Win32::System::Shutdown::{SHUTDOWN_POWEROFF, SHUTDOWN_RESTART};
        assert_eq!(PowerAction::ShutDown.shutdown_flags(), Some(SHUTDOWN_POWEROFF.0));
        assert_eq!(PowerAction::Restart.shutdown_flags(), Some(SHUTDOWN_RESTART.0));
    }

    #[test]
    fn the_flatpak_reaches_every_system_bus_name_nightjar_calls() {
        // nightjar#80: the 4.9.0 Flatpak had no system bus, so logind, NetworkManager and
        // RealtimeKit all failed with "No such file or directory".
        let manifest = include_str!("../packaging/linux/flatpak/io.github.ramazankara.nightjar.yml.in");
        for name in ["org.freedesktop.login1", "org.freedesktop.NetworkManager", "org.freedesktop.RealtimeKit1"] {
            assert!(manifest.lines().any(|l| l.trim() == format!("- --system-talk-name={name}")), "{name}");
        }
        assert!(!manifest.contains("--socket=system-bus"), "only the names Nightjar calls");
    }

    #[test]
    fn each_action_has_its_own_words() {
        for action in PowerAction::ALL {
            let others = PowerAction::ALL.into_iter().filter(|a| *a != action);
            for other in others {
                assert_ne!(action.label(), other.label());
                assert_ne!(action.question(), other.question());
                assert_ne!(action.failed(), other.failed());
            }
        }
    }
}
