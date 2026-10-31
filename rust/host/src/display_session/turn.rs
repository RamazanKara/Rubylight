//! Turning a physical display with the stream, and following a device that turns.
use super::*;

/// How a stream turns its physical display (`dd_rotation_option`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PhysicalTurn {
    /// The rotation in degrees it turns to, or None to leave it.
    pub(super) degrees: Option<u16>,
    /// The resolution asked of the display before the turn, in the shape it
    /// has then.
    pub(super) before: Option<(u32, u32)>,
    /// The resolution the desktop shows once it has turned.
    pub(super) after: Option<(u32, u32)>,
}

/// The turn a stream gives its physical display. `read` gives the display's
/// rotation and desktop size; it is read only when the option can turn the
/// display, and a display that cannot be read fails the stream rather than
/// stream it unturned.
pub(super) fn plan_physical_turn(
    config: &Config,
    stream: (u32, u32),
    resolution: Option<(u32, u32)>,
    read: impl FnOnce() -> Result<(u16, (u32, u32))>,
) -> Result<PhysicalTurn> {
    let unturned = PhysicalTurn {
        degrees: None,
        before: resolution,
        after: resolution,
    };
    let option = config.dd_rotation_option();
    if option == butterpollo_core::config::DisplayAdjust::Disabled {
        return Ok(unturned);
    }
    let (current, desktop) =
        read().context("the display's rotation could not be read for dd_rotation_option")?;
    let Some(degrees) = butterpollo_core::display_policy::physical_rotation(
        option,
        config.dd_manual_rotation(),
        current,
        desktop,
        stream,
    ) else {
        return Ok(unturned);
    };
    let before =
        resolution.map(|size| butterpollo_core::display_policy::mode_before_turn(size, desktop));
    // A quarter turn swaps the desktop's width and height.
    let quarter = (degrees + 360 - current) % 180 == 90;
    Ok(PhysicalTurn {
        degrees: Some(degrees),
        before,
        after: before.map(|(width, height)| {
            if quarter {
                (height, width)
            } else {
                (width, height)
            }
        }),
    })
}

/// What a stream changes on its physical display beside the mode, so tests
/// can stand in for the display lease.
pub(super) trait PhysicalAdjust {
    fn rotate(&mut self, degrees: u16) -> Result<()>;
    fn scale(&mut self, configured: i64) -> Result<()>;
}

impl PhysicalAdjust for Guard {
    fn rotate(&mut self, degrees: u16) -> Result<()> {
        Guard::rotate(self, degrees)
    }
    fn scale(&mut self, configured: i64) -> Result<()> {
        Guard::scale(self, configured)
    }
}

/// Turns, then scales, a stream's physical display. The turn comes first: the
/// display's mode was set in its shape before the turn, and a recommended
/// scale is read off the turned desktop. The lease puts both back.
pub(super) fn adjust_physical(
    lease: &mut impl PhysicalAdjust,
    turn: Option<u16>,
    scale: i64,
) -> Result<()> {
    if let Some(degrees) = turn {
        lease.rotate(degrees)?;
    }
    if scale != 0 {
        lease.scale(scale)?;
    }
    Ok(())
}

/// How a stream's own display follows a stream that turns between portrait
/// and landscape (`dd_rotation_option = auto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Follow {
    /// Its virtual display changes shape.
    Virtual,
    /// Its physical display turns.
    Physical,
}

/// What following a turned stream needs of the display lease, so tests can
/// stand in for it.
pub(super) trait TurnLease: PhysicalAdjust {
    fn virtual_size(&self) -> Option<(u32, u32)>;
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()>;
}

impl TurnLease for Guard {
    fn virtual_size(&self) -> Option<(u32, u32)> {
        Guard::virtual_size(self)
    }
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()> {
        Guard::reshape_virtual(self, width, height)
    }
}

/// Gives the stream's display the shape of a stream that turned to `to`: a
/// virtual display is reshaped, a physical one turned (`read` gives its
/// rotation and desktop size). Returns the desktop size it shows now, or None
/// when the display already has the stream's shape or keeps its own.
pub(super) fn follow_turn(
    lease: &mut impl TurnLease,
    follow: Follow,
    to: (u32, u32),
    mut read: impl FnMut() -> Result<(u16, (u32, u32))>,
) -> Result<Option<(u32, u32)>> {
    use butterpollo_core::config::DisplayAdjust;
    match follow {
        Follow::Virtual => {
            let size = lease.virtual_size().context(
                "the virtual display is shared or not at the stream's size, so it keeps its shape",
            )?;
            let shaped =
                butterpollo_core::display_policy::virtual_shape(DisplayAdjust::Auto, 0, size, to);
            if shaped == size {
                return Ok(None);
            }
            lease.reshape_virtual(shaped.0, shaped.1)?;
            Ok(Some(shaped))
        }
        Follow::Physical => {
            let (current, desktop) =
                read().context("the display's rotation could not be read to follow the stream")?;
            let Some(degrees) = butterpollo_core::display_policy::physical_rotation(
                DisplayAdjust::Auto,
                0,
                current,
                desktop,
                to,
            ) else {
                return Ok(None);
            };
            lease.rotate(degrees)?;
            // A display another stream uses keeps its rotation.
            let (_, turned) = read().context("the turned display could not be read")?;
            Ok((turned != desktop).then_some(turned))
        }
    }
}
