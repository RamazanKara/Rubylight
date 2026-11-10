//! Run on a Mac with Metal and VideoToolbox. Cocoa requires the process main thread.
#![cfg(all(target_os = "macos", feature = "demo-library"))]

#[test]
#[ignore = "requires a Mac display and hardware decoder"]
fn metal_shaders_draw_the_reference_fixtures() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_nightjar"))
        .arg("--check-metal")
        .status()
        .expect("start Nightjar");
    assert!(status.success(), "Metal check exited with {status}");
}
