//! Change one display's HDR state or mode while a stream captures it, for the mid-stream
//! capture checks (a virtual display's surface changing under the frame push):
//!
//! ```text
//! display_poke \\.\DISPLAY9 hdr on|off
//! display_poke \\.\DISPLAY9 mode 1920x1080x120
//! display_poke \\.\DISPLAY9 mode 1920x1080        (keeps the refresh rate)
//! ```
//!
//! The mode change is temporary (`CDS_FULLSCREEN`) and the HDR state is the display's own;
//! the stream's display recovery restores both when it ends. Run it in the signed-in session.
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, what, value] = args.as_slice() else {
        bail!("usage: display_poke DISPLAY hdr on|off | DISPLAY mode WxH[xF]");
    };
    match what.as_str() {
        "hdr" => {
            let monitor = butterpollo_windows::display::monitors()?
                .into_iter()
                .find(|m| m.display_name.eq_ignore_ascii_case(name))
                .with_context(|| format!("no active display {name}"))?;
            butterpollo_windows::display::set_hdr(&monitor, value == "on")?;
        }
        "mode" => {
            let parts: Vec<u32> = value
                .split('x')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .context("mode is WxH or WxHxF")?;
            let (width, height, fps) = match parts[..] {
                [width, height, fps] => (width, height, fps),
                [width, height] => (
                    width,
                    height,
                    butterpollo_windows::display::mode(name)?.dmDisplayFrequency,
                ),
                _ => bail!("mode is WxH or WxHxF"),
            };
            butterpollo_windows::display::set_mode(name, width, height, fps)?;
        }
        _ => bail!("usage: display_poke DISPLAY hdr on|off | DISPLAY mode WxH[xF]"),
    }
    println!("{name}: {what} {value}");
    Ok(())
}
