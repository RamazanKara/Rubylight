//! GPU-free integration matrix through the real HTTP, RTSP, ENet and session
//! workers. Only native preparation/input and capture/encoder output are fake.
//! No service, driver, environment switch or production port is used.
use anyhow::{Context, Result};
use butterpollo_config::config::{Config, Ports};
use butterpollo_crypto::crypto;
use butterpollo_input_policy::controller_audio;
use butterpollo_session::session::{Role, Transport};
use butterpollo_session::{
    mic,
    state::{App, Client as PairedClient},
};
use butterpollo_stream::state::{Launch, Session};
use butterpollo_stream::stream::effective_config;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "reconnect_tests/clipboard_sync_tests.rs"]
mod clipboard_sync_tests;
#[path = "reconnect_tests/control_fuzz_tests.rs"]
mod control_fuzz_tests;
#[path = "reconnect_tests/extension_tests.rs"]
mod extension_tests;
#[path = "reconnect_tests/frame_warp_tests.rs"]
mod frame_warp_tests;
#[path = "reconnect_tests/frame_warp_v2_tests.rs"]
mod frame_warp_v2_tests;
#[path = "reconnect_tests/rate_change_tests.rs"]
mod rate_change_tests;
#[path = "reconnect_tests/roaming_tests.rs"]
mod roaming_tests;
#[path = "reconnect_tests/session_end_tests.rs"]
mod session_end_tests;
#[path = "reconnect_tests/webrtc_tests.rs"]
mod webrtc_tests;

use butterpollo_stream::stream::reconnect_harness::{
    CLAIM, CLOSED, Client, FAILED, Harness, LossySocket, PORT_SLOTS, WAIT, address, claim_slot,
    established, opened_rtsp, port_base, reserve, sealed_rtsp, udp, until, xml,
};

#[path = "reconnect_tests/slot_tests.rs"]
mod slot_tests;

const VIEWPORT: &str =
    "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n";

/// Moves `launch`'s reply `age` into the past, as a client that took that long would see it.
fn age_launch(h: &Harness, launch: &Launch, age: Duration) {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .get_mut(&launch.id)
        .expect("launch pending")
        .created = Instant::now() - age;
}

/// A launch of app `id` from `client`, with the query a Moonlight launch sends.
async fn launch_app(h: &Harness, client: usize, id: u32) -> Result<String> {
    Ok(h.clients[client]
        .get(format!("https://127.0.0.1:{}/launch", h.ports.https))
        .query(&[
            ("appid", id.to_string()),
            ("rikey", hex::encode(crypto::random::<16>())),
            ("rikeyid", "1".into()),
            ("corever", "1".into()),
            ("mode", "1281x721x60".into()),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

/// A harness whose host has Nucleus Co-op (the fake) and Portal 2, which it
/// has a handler for; client 1 may join co-op and client 0 has it turned off.
/// Returns the co-op entry's id.
async fn coop_harness() -> Result<(Harness, Arc<butterpollo_stream::nucleus::fake::Pc>, u32)> {
    let h = Harness::new(10_000, 2).await?;
    let pc = Arc::new(butterpollo_stream::nucleus::fake::Pc::installed());
    h.h.nucleus.use_pc(pc.clone());
    h.h.apps.write().unwrap().push(App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", butterpollo_stream::nucleus::fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    });
    h.h.paired.write().unwrap().clients[1]
        .extra
        .insert("coop_join".into(), true.into());
    // Never set, a device that launches apps may join: client 0 has it off.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), false.into());
    let apps = h.h.apps.read().unwrap().clone();
    let entries = h.h.nucleus.entries(&apps, "");
    assert_eq!(entries.len(), 1);
    let id = entries[0].id();
    Ok((h, pc, id))
}
/// A copy of the launch `client` has pending; it holds the launch's display lease.
fn pending_of(h: &Harness, client: usize) -> Option<Launch> {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .values()
        .find(|l| l.client.uuid == client.to_string())
        .cloned()
}

#[path = "reconnect_tests/background_tests.rs"]
mod background_tests;
#[path = "reconnect_tests/coop_tests.rs"]
mod coop_tests;
#[path = "reconnect_tests/input_notices_tests.rs"]
mod input_notices_tests;
#[path = "reconnect_tests/join_resume_tests.rs"]
mod join_resume_tests;
#[path = "reconnect_tests/media_and_launch_tests.rs"]
mod media_and_launch_tests;
#[path = "reconnect_tests/rtsp_lifecycle_tests.rs"]
mod rtsp_lifecycle_tests;
