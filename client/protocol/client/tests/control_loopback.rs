//! The ENet driver against an in-process rusty_enet host that seals and opens messages with
//! the Rubylight host's own helpers.

#[path = "control_loopback/driver.rs"]
mod driver;
#[path = "control_loopback/host.rs"]
mod host;
#[path = "control_loopback/roaming.rs"]
mod roaming;
