//! `GET /wake`: the PC's wake targets and its sleep state.
use serde::{Deserialize, Serialize};

/// `Check::id` values. Clients show ids they don't know by their `state`.
pub mod check {
    /// "Allow this device to wake the computer" (`MSPower_DeviceWakeEnable`).
    pub const WAKE_ENABLED: &str = "wake_enabled";
    /// The adapter's `*WakeOnMagicPacket` advanced property.
    pub const MAGIC_PACKET: &str = "magic_packet";
    /// "Only allow a magic packet to wake the computer" (`MSNdis_DeviceWakeOnMagicPacketOnly`).
    pub const MAGIC_PACKET_ONLY: &str = "magic_packet_only";
    /// `*WakeOnPattern`. Information only.
    pub const WAKE_ON_PATTERN: &str = "wake_on_pattern";
    /// `*ModernStandbyWoLMagicPacket`, on a Modern Standby PC whose adapter has it.
    pub const MODERN_STANDBY_MAGIC_PACKET: &str = "modern_standby_magic_packet";
    /// A vendor's wake from shutdown (Realtek `S5WakeOnLan` and similar). Information only.
    pub const SHUTDOWN_WAKE: &str = "shutdown_wake";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Ethernet,
    Wifi,
    /// A kind from a newer host.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Ready,
    /// A wake setting is off; the console's Fix button turns it on.
    NeedsSetup,
    /// Wi-Fi: wake depends on the adapter and the access point.
    Experimental,
    /// The settings could not be read. Also a state from a newer host.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Ok,
    Warning,
    Error,
    /// Not readable. Also a state from a newer host.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// One of [`check`], or an id from a newer host.
    pub id: String,
    pub state: CheckState,
}

/// A physical adapter a magic packet can wake the PC through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// `aa:bb:cc:dd:ee:ff`, the format of `<mac>` in `/serverinfo`.
    pub mac: String,
    pub kind: AdapterKind,
    pub connected: bool,
    /// The request that read this came in through this adapter. False for
    /// every target when the client came through a VPN or another virtual
    /// adapter, which a sleeping PC can't be woken through.
    pub route: bool,
    /// IPv4 subnet-directed broadcasts of the adapter.
    #[serde(default)]
    pub broadcasts: Vec<String>,
    pub state: TargetState,
    #[serde(default)]
    pub checks: Vec<Check>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sleep {
    /// The PC can sleep (S3).
    S3,
    /// No S3: the PC can't sleep, or uses Modern Standby only.
    None,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastStartup {
    /// On, and hibernate is available: a shutdown is a hibernation.
    Active,
    /// On, but hibernate is off, so it has no effect.
    Inactive,
    Off,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownWake {
    /// Wake from shutdown depends on firmware settings Windows can't read.
    FirmwareUnknown,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct System {
    pub sleep: Sleep,
    pub modern_standby: bool,
    pub hibernate: bool,
    pub fast_startup: FastStartup,
    pub shutdown_wake: ShutdownWake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleSleepState {
    Off,
    /// The PC sleeps once it has been idle for `minutes`.
    Armed,
    /// On, but something keeps the PC awake; `reason` says what.
    Blocked,
    /// On, but this PC can't do it; `reason` says why.
    Unsupported,
    #[serde(other)]
    Unknown,
}

/// The host's own sleep after idle (`sleep_after_idle_minutes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleSleep {
    pub state: IdleSleepState,
    /// 0 when off.
    pub minutes: u32,
    /// Always present: `null` when `armed` or `off`.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    pub revision: u64,
    /// The MAC `/serverinfo` reports to this client, `null` when no physical
    /// adapter is connected (`/serverinfo` then sends zeros).
    pub primary: Option<String>,
    /// Every physical adapter, connected or not.
    pub targets: Vec<Target>,
    pub system: System,
    pub idle_sleep: IdleSleep,
}
