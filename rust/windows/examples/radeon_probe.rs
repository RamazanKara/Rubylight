//! Read-only ADLX 3D settings probe. Prints JSON; `--markdown` prints the settings table for
//! `docs/hardware-matrix.md` instead. Changes no setting.
#![allow(clippy::print_stdout)]
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let report = butterpollo_windows::radeon::report()?;
    if std::env::args().any(|arg| arg == "--markdown") {
        for gpu in &report.probe.gpus {
            println!("### {} (id {})\n", gpu.name, gpu.id);
            println!(
                "{}",
                butterpollo_windows::radeon::model::markdown_table(&gpu.settings)
            );
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
