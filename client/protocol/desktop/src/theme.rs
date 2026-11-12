//! Nightjar's look on the desktop: the Android app's palettes (the night one's blue-black
//! surfaces and the amber of the brand moon, the day one's light glass, OLED black and high
//! contrast; Android's `AppTheme.Look`), Figtree type, and the brand mark drawn from its vector path.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

use crate::strings::Lang;
use crate::ui::design_tokens as t;

/// A colour role. The palette behind it is the one of the [`Look`] in use (night, day, OLED black
/// or high contrast), so drawing code names a role and never a colour. It derefs to the current
/// `Color32` (`*MOON`) and converts into one where egui takes `impl Into<Color32>`.
#[derive(Clone, Copy, Debug)]
pub struct Role(u8);

impl std::ops::Deref for Role {
    type Target = Color32;
    fn deref(&self) -> &Color32 {
        &palette(look())[usize::from(self.0)]
    }
}

impl From<Role> for Color32 {
    fn from(role: Role) -> Self {
        *role
    }
}

macro_rules! roles {
    ($($name:ident: $normal:expr, $high:expr, $day:expr;)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum Index { $($name),* }
        const COUNT: usize = [$(Index::$name),*].len();
        const NORMAL: [Color32; COUNT] = [$($normal),*];
        const HIGH: [Color32; COUNT] = [$($high),*];
        const DAY: [Color32; COUNT] = [$($day),*];
        // The whole Android and console role set; a role no page draws yet still has its
        // contrast checked by the tests.
        $(#[allow(dead_code)] pub const $name: Role = Role(Index::$name as u8);)*
    };
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// The design's roles, from `tokens.json` through [`t`]. The second colour is the high-contrast one:
// black surfaces, white text, and no grey text at all (the variant and outline roles are on-surface).
// The third is the day one, Android's `values-notnight` roles (`palette.xml`): white and pale grey
// containers, the moon's dark ink for the action with white on it, and dark text throughout. OLED
// black is the night palette with black surfaces ([`OLED`]).
roles! {
    NIGHT: t::COLOR_NIGHT, t::HC_COLOR_NIGHT, t::DAY_COLOR_NIGHT;
    SURFACE_LOWEST: t::COLOR_SURFACE_LOWEST, rgb(0x000000), rgb(0xFFFFFF);
    SURFACE_LOW: t::COLOR_SURFACE_LOW, rgb(0x0A0A0C), rgb(0xF2F3F8);
    SURFACE: t::COLOR_SURFACE, rgb(0x101114), rgb(0xECEDF2);
    SURFACE_HIGH: t::COLOR_SURFACE_HIGH, rgb(0x1C1D22), rgb(0xE6E7EC);
    SURFACE_HIGHEST: t::COLOR_SURFACE_HIGHEST, rgb(0x2A2C33), rgb(0xE1E2E8);
    // One step past the highest: a hovered or pressed control that already sits on SURFACE_HIGHEST.
    // By day the ladder climbs toward grey, so a hover darkens a surface a little.
    SURFACE_BRIGHT: t::COLOR_SURFACE_BRIGHT, rgb(0x383A42), rgb(0xD8D9DF);
    ON_SURFACE: t::COLOR_LABEL, t::HC_COLOR_LABEL, t::DAY_COLOR_LABEL;
    ON_SURFACE_VARIANT: t::COLOR_LABEL_SECONDARY, t::HC_COLOR_LABEL_SECONDARY, t::DAY_COLOR_LABEL_SECONDARY;
    // Text on hovered cards and buttons too (4.5:1 on SURFACE_HIGHEST), lighter than Android's at
    // night and darker by day.
    OUTLINE: rgb(0xA0A3B1), rgb(0xFFFFFF), rgb(0x5B5F6E);
    OUTLINE_VARIANT: rgb(0x3F4250), t::HC_COLOR_RIM, rgb(0xC5C7D3);
    MOON: t::COLOR_MOON, t::HC_COLOR_MOON, t::DAY_COLOR_MOON_INK;
    ON_MOON: t::COLOR_ON_MOON, t::HC_COLOR_ON_MOON, rgb(0xFFFFFF);
    MOON_CONTAINER: rgb(0x5C3E00), rgb(0x5C3E00), rgb(0xFFDDB3);
    ON_MOON_CONTAINER: t::COLOR_ON_SELECTED, rgb(0xFFFFFF), t::DAY_COLOR_ON_SELECTED;
    TERTIARY: rgb(0xAFC8E9), rgb(0xCFE2FF), rgb(0x3F5F80);
    ERROR: t::COLOR_ERROR, t::HC_COLOR_ERROR, t::DAY_COLOR_ERROR;
    // A failure that fills a shape and its text.
    ERROR_CONTAINER: t::COLOR_DANGER_FILL, rgb(0x93000A), t::DAY_COLOR_DANGER_FILL;
    ON_ERROR_CONTAINER: t::COLOR_ON_DANGER_FILL, rgb(0xFFFFFF), t::DAY_COLOR_ON_DANGER_FILL;
    // Status: up, ready, running; and something to look at. Warn is yellow because amber is the
    // action, and it is never a filled shape: an icon and a word.
    OK: t::COLOR_OK, t::HC_COLOR_OK, t::DAY_COLOR_OK;
    WARN: t::COLOR_WARN, t::HC_COLOR_WARN, t::DAY_COLOR_WARN;
    // The orbit rings of the site banner.
    ORBIT: t::COLOR_ORBIT, rgb(0x2A2D3A), rgb(0xD3D6E0);
    // Paper white of the wordmark; its ink by day.
    PAPER: t::COLOR_PAPER, rgb(0xFFFFFF), t::DAY_COLOR_PAPER;
}

/// OLED black: the night palette on black, as Android's `OledBlackTheme` makes every surface
/// black. The steps a hover and a press climb to stay grey enough to see.
const OLED: [Color32; COUNT] = {
    let mut p = NORMAL;
    p[Index::NIGHT as usize] = t::OLED_COLOR_NIGHT;
    p[Index::SURFACE_LOWEST as usize] = rgb(0x000000);
    p[Index::SURFACE_LOW as usize] = rgb(0x000000);
    p[Index::SURFACE as usize] = rgb(0x000000);
    p[Index::SURFACE_HIGH as usize] = t::OLED_COVER_PLACEHOLDER;
    p[Index::SURFACE_HIGHEST as usize] = rgb(0x24252B);
    p[Index::SURFACE_BRIGHT as usize] = rgb(0x31333A);
    p
};

/// What text over game art sits on (the console's --media-bg, Android's `tv_chip_scrim`), the same
/// in every palette: white on it over a white frame is still 9:1.
pub const SCRIM: Color32 = t::COLOR_SCRIM;

/// The design's 4-based spacing scale, in points.
#[allow(dead_code, reason = "the whole scale, so a new page picks a step instead of a literal")]
pub mod space {
    use crate::ui::design_tokens as t;
    pub const XS: f32 = t::SPACE_XS;
    pub const S: f32 = t::SPACE_S;
    pub const M: f32 = t::SPACE_M;
    pub const L: f32 = t::SPACE_L;
    pub const XL: f32 = t::SPACE_XL;
    pub const XXL: f32 = t::SPACE_XXL;
    pub const XXXL: f32 = t::SPACE_XXXL;
}

/// The design's type scale, in points.
pub mod text {
    use crate::ui::design_tokens as t;
    pub const LARGE_TITLE: f32 = t::TYPE_LARGE_TITLE_SIZE;
    pub const TITLE: f32 = t::TYPE_TITLE_SIZE;
    pub const SECTION: f32 = t::TYPE_SECTION_SIZE;
    pub const CARD_TITLE: f32 = t::TYPE_HEADLINE_SIZE;
    pub const BODY: f32 = t::TYPE_BODY_SIZE;
    pub const LABEL: f32 = t::TYPE_SUB_SIZE;
    pub const CAPTION: f32 = t::TYPE_CAPTION_SIZE;
}

/// The design's corners: small tags and tiles, then buttons, fields and menus, then groups, cards
/// and dialogs.
#[allow(dead_code, reason = "the whole scale, so a new page picks a step instead of a literal")]
pub mod radius {
    use crate::ui::design_tokens as t;
    pub const SMALL: u8 = t::RADIUS_TILE as u8;
    pub const CONTROL: u8 = t::RADIUS_CONTROL as u8;
    pub const PANEL: u8 = t::RADIUS_GROUP as u8;
}

/// Height of primary actions and list rows (the console's --control-height); dense inline controls
/// keep `interact_size.y`.
pub const CONTROL_HEIGHT: f32 = 44.0;

fn palette(look: Look) -> &'static [Color32; COUNT] {
    match look {
        Look::Night => &NORMAL,
        Look::Day => &DAY,
        Look::Oled => &OLED,
        Look::HighContrast => &HIGH,
    }
}

/// The look of the launcher's pages, Android's `AppTheme.Look`: the design's tokens come in these
/// sets. High contrast wins over the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Look {
    /// The night palette and sky: the look Nightjar always had.
    #[default]
    Night,
    /// Light glass on a pale sky with a low sun and no stars.
    Day,
    /// The night look on black: black canvas, sky and surfaces.
    Oled,
    /// Black surfaces, white text, solid glass and thick focus rings.
    HighContrast,
}

impl Look {
    /// The look for the Theme setting: System follows the system's light or dark setting
    /// (`system_light`), as Android's `AppTheme.look`; high contrast wins over every theme.
    pub fn of(theme: crate::config::Theme, system_light: bool, high: bool) -> Self {
        use crate::config::Theme;
        match theme {
            _ if high => Self::HighContrast,
            Theme::System if system_light => Self::Day,
            Theme::System | Theme::Dark => Self::Night,
            Theme::Oled => Self::Oled,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Day,
            2 => Self::Oled,
            3 => Self::HighContrast,
            _ => Self::Night,
        }
    }
}

static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);
/// The theme's look under high contrast: night, day or OLED black.
static BASE_LOOK: AtomicU8 = AtomicU8::new(Look::Night as u8);
/// The coming frames draw over a stream's video ([`set_over_video`]).
static OVER_VIDEO: AtomicBool = AtomicBool::new(false);
/// Whether reduced motion was asked for at the last [`set_look`], for the style a change of
/// [`set_over_video`] sets again.
static STILL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    /// A test's own palette, so a test looking at high contrast does not recolour the others.
    static TEST_HIGH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// The same for the theme's look.
    static TEST_BASE: std::cell::Cell<Option<Look>> = const { std::cell::Cell::new(None) };
    /// And for drawing over video.
    static TEST_OVER: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn high_contrast() -> bool {
    #[cfg(test)]
    if let Some(high) = TEST_HIGH.with(std::cell::Cell::get) {
        return high;
    }
    HIGH_CONTRAST.load(Ordering::Relaxed)
}

pub fn set_high_contrast(on: bool) {
    // A test with its own palette changes only that, never what the other tests see.
    #[cfg(test)]
    if TEST_HIGH.with(|h| h.get().is_some()) {
        TEST_HIGH.with(|h| h.set(Some(on)));
        return;
    }
    HIGH_CONTRAST.store(on, Ordering::Relaxed);
}

/// The theme's look, high contrast aside.
fn base_look() -> Look {
    #[cfg(test)]
    if let Some(look) = TEST_BASE.with(std::cell::Cell::get) {
        return look;
    }
    Look::from_u8(BASE_LOOK.load(Ordering::Relaxed))
}

fn set_base_look(look: Look) {
    let look = if look == Look::HighContrast { Look::Night } else { look };
    #[cfg(test)]
    if TEST_BASE.with(|b| b.get().is_some()) {
        TEST_BASE.with(|b| b.set(Some(look)));
        return;
    }
    BASE_LOOK.store(look as u8, Ordering::Relaxed);
}

/// The look drawing uses now. Over a stream's video the stream menu and overlay keep the night
/// look whatever the theme (Android's stream is always dark); high contrast stays high contrast.
pub fn look() -> Look {
    if high_contrast() {
        Look::HighContrast
    } else if over_video() {
        Look::Night
    } else {
        base_look()
    }
}

fn over_video() -> bool {
    #[cfg(test)]
    if let Some(over) = TEST_OVER.with(std::cell::Cell::get) {
        return over;
    }
    OVER_VIDEO.load(Ordering::Relaxed)
}

/// Sets whether frames draw over video; answers whether they did.
fn swap_over_video(over_video: bool) -> bool {
    #[cfg(test)]
    if TEST_OVER.with(|o| o.get().is_some()) {
        return TEST_OVER.with(|o| o.replace(Some(over_video))).unwrap_or(false);
    }
    OVER_VIDEO.swap(over_video, Ordering::Relaxed)
}

/// Test only: gives this thread its own palette setting, night, not high contrast and not over
/// video at first.
#[cfg(test)]
pub fn own_palette() {
    TEST_HIGH.with(|h| h.set(Some(false)));
    TEST_BASE.with(|b| b.set(Some(Look::Night)));
    TEST_OVER.with(|o| o.set(Some(false)));
}

/// Test only: this thread's palette (see [`own_palette`]) in `look`.
#[cfg(test)]
pub fn own_look(look: Look) {
    own_palette();
    set_high_contrast(look == Look::HighContrast);
    set_base_look(look);
}

/// Whether the coming frames draw over a stream's video (`Gui::set_over_video`): they take the
/// night look, and the launcher's comes back with its frames. The style egui draws its own widgets
/// with is set again only when the colours change.
pub fn set_over_video(ctx: &egui::Context, over_video: bool) {
    let before = look();
    if swap_over_video(over_video) != over_video && look() != before {
        apply_style(ctx, look(), STILL.load(Ordering::Relaxed));
    }
}

/// A design token with its own value by day and on OLED black (`DAY_*` and `OLED_*` in
/// `tokens.json`): [`Shade::get`] gives the one of the look in use. High contrast, which draws its
/// own solid look where it differs, and frames over video get the night value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shade {
    night: Color32,
    day: Color32,
    oled: Color32,
}

impl Shade {
    /// A token whose OLED value is the night one.
    pub const fn new(night: Color32, day: Color32) -> Self {
        Self { night, day, oled: night }
    }

    /// A token with an OLED value of its own.
    pub const fn with_oled(night: Color32, day: Color32, oled: Color32) -> Self {
        Self { night, day, oled }
    }

    pub fn get(self) -> Color32 {
        self.of(look())
    }

    pub fn of(self, look: Look) -> Color32 {
        match look {
            Look::Day => self.day,
            Look::Oled => self.oled,
            Look::Night | Look::HighContrast => self.night,
        }
    }
}

impl From<Shade> for Color32 {
    fn from(shade: Shade) -> Self {
        shade.get()
    }
}

/// The design tokens that change with the look, named as in [`t`]; the other tokens are the same
/// in every look and stay there.
#[allow(dead_code, reason = "the whole set of tokens that change by day, so a page picks one up")]
pub mod shade {
    use super::{rgb, Shade};
    use crate::ui::design_tokens as t;

    pub const COLOR_CANVAS: Shade = Shade::with_oled(t::COLOR_CANVAS, t::DAY_COLOR_CANVAS, t::OLED_COLOR_CANVAS);
    pub const COLOR_ON_SELECTED: Shade = Shade::new(t::COLOR_ON_SELECTED, t::DAY_COLOR_ON_SELECTED);
    pub const COLOR_MOON_INK: Shade = Shade::new(t::COLOR_MOON_INK, t::DAY_COLOR_MOON_INK);
    pub const COLOR_OK: Shade = Shade::new(t::COLOR_OK, t::DAY_COLOR_OK);
    pub const COLOR_WARN: Shade = Shade::new(t::COLOR_WARN, t::DAY_COLOR_WARN);
    pub const COLOR_ERROR: Shade = Shade::new(t::COLOR_ERROR, t::DAY_COLOR_ERROR);
    pub const COLOR_OFFLINE: Shade = Shade::new(t::COLOR_OFFLINE, t::DAY_COLOR_OFFLINE);
    pub const COLOR_DANGER_FILL: Shade = Shade::new(t::COLOR_DANGER_FILL, t::DAY_COLOR_DANGER_FILL);
    pub const COLOR_ON_DANGER_FILL: Shade = Shade::new(t::COLOR_ON_DANGER_FILL, t::DAY_COLOR_ON_DANGER_FILL);
    pub const COLOR_PAGE_DIM: Shade = Shade::new(t::COLOR_PAGE_DIM, t::DAY_COLOR_PAGE_DIM);
    /// The rows' ground, Android's `surface_container_low`: black on OLED.
    pub const COLOR_SURFACE_LOW: Shade = Shade::with_oled(t::COLOR_SURFACE_LOW, rgb(0xF2F3F8), rgb(0x000000));
    pub const SKY_TOP: Shade = Shade::with_oled(t::SKY_TOP, t::DAY_SKY_TOP, t::OLED_SKY_TOP);
    pub const SKY_MID: Shade = Shade::with_oled(t::SKY_MID, t::DAY_SKY_MID, t::OLED_SKY_MID);
    pub const SKY_BOTTOM: Shade = Shade::with_oled(t::SKY_BOTTOM, t::DAY_SKY_BOTTOM, t::OLED_SKY_BOTTOM);
    pub const SKY_GLOW: Shade = Shade::new(t::SKY_GLOW, t::DAY_SKY_GLOW);
    pub const SKY_RING: Shade = Shade::new(t::SKY_RING, t::DAY_SKY_RING);
    pub const FOCUS_RING_COLOR: Shade = Shade::new(t::FOCUS_RING_COLOR, t::DAY_FOCUS_RING_COLOR);
    pub const FOCUS_RING_COLOR_ON_MOON: Shade = Shade::new(t::FOCUS_RING_COLOR_ON_MOON, t::DAY_FOCUS_RING_COLOR_ON_MOON);
    pub const FOCUS_GAP_COLOR: Shade = Shade::new(t::FOCUS_GAP_COLOR, t::DAY_FOCUS_GAP_COLOR);
    pub const FOCUS_HALO_COLOR: Shade = Shade::new(t::FOCUS_HALO_COLOR, t::DAY_FOCUS_HALO_COLOR);
    pub const FOCUS_FILL_LIFT: Shade = Shade::new(t::FOCUS_FILL_LIFT, t::DAY_FOCUS_FILL_LIFT);
    pub const HOVER_FILL_LIFT: Shade = Shade::new(t::HOVER_FILL_LIFT, t::DAY_HOVER_FILL_LIFT);
    pub const PRESS_FILL_LIFT: Shade = Shade::new(t::PRESS_FILL_LIFT, t::DAY_PRESS_FILL_LIFT);
    pub const TINT_GROUP_FILL: Shade = Shade::with_oled(t::TINT_GROUP_FILL, t::DAY_TINT_GROUP_FILL, t::OLED_TINT_GROUP_FILL);
    pub const TINT_GROUP_RIM_TOP: Shade = Shade::new(t::TINT_GROUP_RIM_TOP, t::DAY_TINT_GROUP_RIM_TOP);
    pub const TINT_GROUP_RIM_MID: Shade = Shade::new(t::TINT_GROUP_RIM_MID, t::DAY_TINT_GROUP_RIM_MID);
    pub const TINT_GROUP_RIM_BOTTOM: Shade = Shade::new(t::TINT_GROUP_RIM_BOTTOM, t::DAY_TINT_GROUP_RIM_BOTTOM);
    pub const TINT_CONTROL_FILL: Shade = Shade::new(t::TINT_CONTROL_FILL, t::DAY_TINT_CONTROL_FILL);
    pub const TINT_CONTROL_RIM_TOP: Shade = Shade::new(t::TINT_CONTROL_RIM_TOP, t::DAY_TINT_CONTROL_RIM_TOP);
    pub const TINT_CONTROL_RIM_MID: Shade = Shade::new(t::TINT_CONTROL_RIM_MID, t::DAY_TINT_CONTROL_RIM_MID);
    pub const TINT_CONTROL_RIM_BOTTOM: Shade = Shade::new(t::TINT_CONTROL_RIM_BOTTOM, t::DAY_TINT_CONTROL_RIM_BOTTOM);
    pub const TINT_SELECTED_FILL: Shade = Shade::new(t::TINT_SELECTED_FILL, t::DAY_TINT_SELECTED_FILL);
    pub const TINT_SELECTED_RIM_TOP: Shade = Shade::new(t::TINT_SELECTED_RIM_TOP, t::DAY_TINT_SELECTED_RIM_TOP);
    pub const TINT_GROUP_ON_ART_FILL: Shade = Shade::new(t::TINT_GROUP_ON_ART_FILL, t::DAY_TINT_GROUP_ON_ART_FILL);
    pub const TINT_GROUP_ON_SKY_FILL: Shade = Shade::with_oled(t::TINT_GROUP_ON_SKY_FILL, t::DAY_TINT_GROUP_ON_SKY_FILL, t::OLED_TINT_GROUP_ON_SKY_FILL);
    pub const TINT_SEPARATOR: Shade = Shade::new(t::TINT_SEPARATOR, t::DAY_TINT_SEPARATOR);
    pub const TINT_TRACK_FILL: Shade = Shade::new(t::TINT_TRACK_FILL, t::DAY_TINT_TRACK_FILL);
    pub const TINT_SEG_ON_FILL: Shade = Shade::new(t::TINT_SEG_ON_FILL, t::DAY_TINT_SEG_ON_FILL);
    pub const TINT_FADE: Shade = Shade::with_oled(t::TINT_FADE, t::DAY_TINT_FADE, t::OLED_TINT_FADE);
    pub const PLAY_GLOW: Shade = Shade::new(t::PLAY_GLOW, t::DAY_PLAY_GLOW);
    pub const PLAY_RIM_BOTTOM: Shade = Shade::new(t::PLAY_RIM_BOTTOM, t::DAY_PLAY_RIM_BOTTOM);
    pub const COVER_RIM_TOP: Shade = Shade::new(t::COVER_RIM_TOP, t::DAY_COVER_RIM_TOP);
    pub const COVER_RIM_BOTTOM: Shade = Shade::new(t::COVER_RIM_BOTTOM, t::DAY_COVER_RIM_BOTTOM);
    pub const COVER_SHADOW: Shade = Shade::new(t::COVER_SHADOW, t::DAY_COVER_SHADOW);
    pub const COVER_PLACEHOLDER: Shade = Shade::with_oled(t::COVER_PLACEHOLDER, t::DAY_COVER_PLACEHOLDER, t::OLED_COVER_PLACEHOLDER);
}

/// The thumb of a switch or slider: paper white, white by day too (where the paper role is the
/// wordmark's dark ink).
pub fn knob() -> Color32 {
    if look() == Look::Day { Color32::WHITE } else { *PAPER }
}

/// The stroke of the focus ring: thick enough to find from across a room, thicker still in high
/// contrast.
pub fn focus_ring_width() -> f32 {
    if high_contrast() { t::HC_FOCUS_RING_WIDTH } else { t::FOCUS_RING_WIDTH }
}


/// The surface ladder, darkest first. Hover lifts a surface one step, pressing two.
const LADDER: [Role; 6] = [SURFACE_LOWEST, SURFACE_LOW, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST, SURFACE_BRIGHT];

/// One step lighter on the surface ladder (the brightest stays): what a hovered surface fills with.
/// A role off the ladder (the night background) steps to the lowest card surface.
pub fn lighter(role: Role) -> Role {
    match LADDER.iter().position(|r| r.0 == role.0) {
        Some(at) => LADDER[(at + 1).min(LADDER.len() - 1)],
        None => SURFACE,
    }
}

/// `color` moved `t` of the way toward `toward`: how an accent fill (the amber of a primary button)
/// lightens on hover, as the surfaces step up their ladder.
pub fn tint(color: Color32, toward: Color32, t: f32) -> Color32 {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(mix(color.r(), toward.r()), mix(color.g(), toward.g()), mix(color.b(), toward.b()), color.a())
}

/// The fill of a surface `base` in its interaction state: one step lighter while hovered or
/// focused, two while pressed. Focus adds the ring on top ([`crate::ui::focus_ring`]).
pub fn state_fill(base: Role, hovered: bool, pressed: bool) -> Role {
    if pressed {
        lighter(lighter(base))
    } else if hovered {
        lighter(base)
    } else {
        base
    }
}

/// Whether the operating system asks for less motion. Windows: "Show animations in Windows" is
/// off (`SPI_GETCLIENTAREAANIMATION`); macOS: Reduce motion is on. Linux exposes no setting we
/// read, so motion stays. Asked again at most every two seconds; the answer is cheap but frames
/// are many. Tests draw the same frames whatever the machine's setting (macOS runners reduce motion).
pub fn system_reduces_motion() -> bool {
    if cfg!(test) {
        return false;
    }
    static CHECKED: OnceLock<Instant> = OnceLock::new();
    static LAST_MS: AtomicU64 = AtomicU64::new(u64::MAX);
    static ANSWER: AtomicBool = AtomicBool::new(false);
    let now = CHECKED.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let last = LAST_MS.load(Ordering::Relaxed);
    if last == u64::MAX || now.saturating_sub(last) >= 2000 {
        ANSWER.store(os_reduces_motion(), Ordering::Relaxed);
        LAST_MS.store(now, Ordering::Relaxed);
    }
    ANSWER.load(Ordering::Relaxed)
}

#[cfg(windows)]
fn os_reduces_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut animations = 1i32;
    // SAFETY: the pointer is to a live BOOL-sized integer, which this action writes.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut animations as *mut i32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && animations == 0
}

#[cfg(target_os = "macos")]
fn os_reduces_motion() -> bool {
    // AppKit exposes this display preference without asking for accessibility control.
    unsafe { objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() }
}

#[cfg(target_os = "linux")]
fn os_reduces_motion() -> bool {
    false
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

const FIGTREE: &[u8] = include_bytes!("../assets/Figtree-400.ttf");
const FIGTREE_SEMIBOLD: &[u8] = include_bytes!("../assets/Figtree-600.ttf");

/// The language whose system faces the context's fonts hold ([`fonts_for`]).
fn fonts_key() -> egui::Id {
    egui::Id::unique("nightjar-fonts-for")
}

/// Figtree and egui's fonts, and the style. [`fonts_for`] adds the system faces a language needs.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(definitions(SystemFonts::default()));
    ctx.data_mut(|d| d.remove::<Lang>(fonts_key()));
    set_look(ctx, high_contrast(), false);
}

/// Puts the system faces that draw `lang`'s letters beside Figtree. Cheap unless the language
/// changed since the last call, so it runs every frame. The faces load on a thread the first
/// time (a CJK file is 12-36 MB, Linux asks fc-match); until then the fonts in use stay.
pub fn fonts_for(ctx: &egui::Context, lang: Lang) {
    if ctx.data(|d| d.get_temp::<Lang>(fonts_key())) == Some(lang) {
        return;
    }
    let Some(system) = loaded(ctx, lang) else {
        // The loader requests a repaint too, but the launcher's loop sleeps until an SDL event.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    ctx.data_mut(|d| d.insert_temp(fonts_key(), lang));
    ctx.set_fonts(definitions(system));
    // egui holds the faces in use now; another language's files (up to 38 MB each) go.
    LOADED.lock().unwrap_or_else(PoisonError::into_inner).retain(|e| e.0 == lang);
}

/// The system faces loaded for a language and not yet replaced by another's.
static LOADED: Mutex<Vec<(Lang, SystemFonts)>> = Mutex::new(Vec::new());
/// The languages a thread loads faces for.
static LOADING: Mutex<Vec<Lang>> = Mutex::new(Vec::new());

/// Takes a language off `LOADING` when its loader ends, panicking or not. A loader that panicked
/// leaves the language without system faces, so the UI stops waiting instead of retrying.
struct Loading(Lang);

impl Drop for Loading {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("nightjar: loading the {} fonts failed; drawing without them", self.0.name());
            LOADED.lock().unwrap_or_else(PoisonError::into_inner).push((self.0, SystemFonts::default()));
        }
        LOADING.lock().unwrap_or_else(PoisonError::into_inner).retain(|l| *l != self.0);
    }
}

/// `lang`'s system faces when they are loaded; else starts loading them and returns None.
fn loaded(ctx: &egui::Context, lang: Lang) -> Option<SystemFonts> {
    if fallback(lang).is_none() {
        return Some(SystemFonts::default());
    }
    if let Some((_, system)) = LOADED.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|e| e.0 == lang) {
        return Some(system.clone());
    }
    let mut loading = LOADING.lock().unwrap_or_else(PoisonError::into_inner);
    if !loading.contains(&lang) {
        loading.push(lang);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("nightjar-fonts".into()).spawn(move || {
            let _loading = Loading(lang);
            let system = system_fonts(lang);
            LOADED.lock().unwrap_or_else(PoisonError::into_inner).push((lang, system));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            eprintln!("nightjar: could not start the font loader ({e}); loading the {} fonts here", lang.name());
            loading.retain(|l| *l != lang);
            let system = system_fonts(lang);
            LOADED.lock().unwrap_or_else(PoisonError::into_inner).push((lang, system.clone()));
            return Some(system);
        }
    }
    None
}

/// Figtree has Latin-1 only: no Cyrillic, no Polish, Turkish or Vietnamese letters, no CJK.
/// egui's own Ubuntu-Light draws some of them in a lighter weight, and none of CJK.
#[derive(Clone, Default)]
struct SystemFonts {
    /// Ahead of Figtree, so each word comes from one face: the language's words mix Figtree's
    /// letters with ones it lacks (Polish, Turkish, Vietnamese) or are Cyrillic, whose digits
    /// and punctuation would otherwise be Figtree's.
    ahead: bool,
    regular: Option<FontData>,
    semibold: Option<FontData>,
}

fn definitions(system: SystemFonts) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("figtree".into(), Arc::new(FontData::from_static(FIGTREE)));
    fonts.font_data.insert("figtree-semibold".into(), Arc::new(FontData::from_static(FIGTREE_SEMIBOLD)));
    let egui_fonts = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    // egui panics on a face it cannot parse, so one that slipped through is dropped here.
    let usable = |data: Option<FontData>| {
        data.filter(|data| {
            let ok = skrifa::FontRef::from_index((*data.font).as_ref(), data.index).is_ok();
            if !ok {
                eprintln!("nightjar: a system font (face {}) cannot be read; drawing without it", data.index);
            }
            ok
        })
    };
    let (regular, semibold) = (usable(system.regular), usable(system.semibold));
    // A language without a semibold system face draws its bold text with the regular one.
    let semibold = semibold.or_else(|| regular.clone());
    let regular = regular.map(|data| {
        fonts.font_data.insert("system".into(), Arc::new(data));
        "system"
    });
    let semibold = semibold.map(|data| {
        fonts.font_data.insert("system-semibold".into(), Arc::new(data));
        "system-semibold"
    });
    let family = |figtree: &str, system_face: Option<&str>| {
        let mut list: Vec<String> = Vec::new();
        let system_face = system_face.map(str::to_owned);
        if system.ahead {
            list.extend(system_face.clone());
        }
        list.push(figtree.to_owned());
        if !system.ahead {
            list.extend(system_face);
        }
        list.extend(egui_fonts.iter().cloned());
        // Hack, egui's monospace font, comes last for the PlayStation button symbols (glyphs.rs)
        // that neither Figtree nor egui's proportional fonts draw.
        list.push("Hack".into());
        list
    };
    let proportional = family("figtree", regular);
    let bold = family("figtree-semibold", semibold);
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name("semibold".into()), bold);
    fonts
}

/// The system faces for `lang`'s letters, the first found of each list. Says so in the log when
/// a language that needs one finds none: its text then shows in boxes or in a mix of weights.
fn system_fonts(lang: Lang) -> SystemFonts {
    let Some(fallback) = fallback(lang) else {
        return SystemFonts::default();
    };
    let regular = fallback.regular.iter().find_map(load);
    let semibold = fallback.semibold.iter().find_map(load);
    if regular.is_none() {
        eprintln!(
            "nightjar: no system font draws {}; its text will show as boxes or in mixed weights. Install {}",
            lang.name(),
            faces::INSTALL[fallback.install]
        );
    }
    SystemFonts { ahead: fallback.ahead, regular, semibold }
}

/// Where a language's letters come from when Figtree has not got them.
struct Fallback {
    ahead: bool,
    regular: &'static [Face],
    semibold: &'static [Face],
    /// What to install, in [`faces::INSTALL`], when none is found.
    install: usize,
}

fn fallback(lang: Lang) -> Option<Fallback> {
    let (ahead, regular, semibold, install) = match lang {
        Lang::En | Lang::De | Lang::Es | Lang::Fr | Lang::It | Lang::PtBr => return None,
        Lang::Pl | Lang::Ru | Lang::Tr | Lang::Uk | Lang::Vi => (true, faces::SANS, faces::SANS_SEMIBOLD, 0),
        Lang::Ja => (false, faces::JA, faces::JA_SEMIBOLD, 1),
        Lang::Ko => (false, faces::KO, faces::KO_SEMIBOLD, 1),
        Lang::ZhHans => (false, faces::ZH_HANS, faces::ZH_HANS_SEMIBOLD, 1),
        Lang::ZhHant => (false, faces::ZH_HANT, faces::ZH_HANT_SEMIBOLD, 1),
    };
    Some(Fallback { ahead, regular, semibold, install })
}

/// A face in Windows' font folder: its file, and the family and style in its name table, which
/// pick it out of a collection (.ttc).
#[cfg(windows)]
type Face = (&'static str, &'static str, &'static str);

/// A fontconfig pattern and the family the match must have: fc-match answers every pattern with
/// some font.
#[cfg(target_os = "linux")]
type Face = (&'static str, &'static str);

/// Windows 10 and 11 ship Segoe UI, Yu Gothic, Malgun Gothic, YaHei and JhengHei; Meiryo
/// comes with the optional Japanese Supplemental Fonts, so it is only the second choice. The UI cuts (Yu Gothic UI, YaHei UI, JhengHei UI) have
/// the tighter line height menus want.
#[cfg(windows)]
mod faces {
    use super::Face;
    pub const SANS: &[Face] = &[("segoeui.ttf", "Segoe UI", "Regular")];
    pub const SANS_SEMIBOLD: &[Face] = &[("seguisb.ttf", "Segoe UI Semibold", "Regular")];
    pub const JA: &[Face] = &[("YuGothM.ttc", "Yu Gothic UI", "Regular"), ("meiryo.ttc", "Meiryo UI", "Regular"), ("meiryo.ttc", "Meiryo", "Regular")];
    pub const JA_SEMIBOLD: &[Face] = &[("YuGothB.ttc", "Yu Gothic UI Semibold", "Regular"), ("meiryob.ttc", "Meiryo UI", "Bold"), ("meiryob.ttc", "Meiryo", "Bold")];
    pub const KO: &[Face] = &[("malgun.ttf", "Malgun Gothic", "Regular")];
    pub const KO_SEMIBOLD: &[Face] = &[("malgunbd.ttf", "Malgun Gothic", "Bold")];
    pub const ZH_HANS: &[Face] = &[("msyh.ttc", "Microsoft YaHei UI", "Regular"), ("msyh.ttc", "Microsoft YaHei", "Regular")];
    pub const ZH_HANS_SEMIBOLD: &[Face] = &[("msyhbd.ttc", "Microsoft YaHei UI", "Bold"), ("msyhbd.ttc", "Microsoft YaHei", "Bold")];
    pub const ZH_HANT: &[Face] = &[("msjh.ttc", "Microsoft JhengHei UI", "Regular"), ("msjh.ttc", "Microsoft JhengHei", "Regular")];
    pub const ZH_HANT_SEMIBOLD: &[Face] = &[("msjhbd.ttc", "Microsoft JhengHei UI", "Bold"), ("msjhbd.ttc", "Microsoft JhengHei", "Bold")];
    pub const INSTALL: [&str; 2] = ["Segoe UI (Windows' own font).", "the language's supplemental fonts (Settings > Apps > Optional features)."];
}

#[cfg(target_os = "macos")]
type Face = &'static str;

#[cfg(target_os = "macos")]
mod faces {
    use super::Face;
    pub const SANS: &[Face] = &["HelveticaNeue", "ArialMT"];
    pub const SANS_SEMIBOLD: &[Face] = &["HelveticaNeue-Medium", "Arial-BoldMT"];
    pub const JA: &[Face] = &["HiraginoSans-W3"];
    pub const JA_SEMIBOLD: &[Face] = &["HiraginoSans-W6"];
    pub const KO: &[Face] = &["AppleSDGothicNeo-Regular"];
    pub const KO_SEMIBOLD: &[Face] = &["AppleSDGothicNeo-SemiBold"];
    pub const ZH_HANS: &[Face] = &["PingFangSC-Regular", "STHeitiSC-Light"];
    pub const ZH_HANS_SEMIBOLD: &[Face] = &["PingFangSC-Semibold", "STHeitiSC-Medium"];
    pub const ZH_HANT: &[Face] = &["PingFangTC-Regular", "STHeitiTC-Light"];
    pub const ZH_HANT_SEMIBOLD: &[Face] = &["PingFangTC-Semibold", "STHeitiTC-Medium"];
    pub const INSTALL: [&str; 2] = ["the system fonts in Font Book.", "the language's fonts in Font Book."];
}

#[cfg(target_os = "macos")]
fn load(&name: &Face) -> Option<FontData> {
    use skrifa::{MetadataProvider, string::StringId};
    let font = core_text::font::new_from_name(name, 14.0).ok()?;
    if font.postscript_name() != name { return None; }
    let bytes = std::fs::read(font.url()?.to_path()?).ok()?;
    let index = skrifa::raw::FileRef::new(&bytes).ok()?.fonts().position(|face| {
        face.is_ok_and(|face| face.localized_strings(StringId::POSTSCRIPT_NAME)
            .any(|s| s.to_string() == name))
    })?;
    face_data(bytes, index.try_into().ok()?)
}

/// Noto ships in most distributions (fonts-noto-core, fonts-noto-cjk); DejaVu is the older default.
#[cfg(target_os = "linux")]
mod faces {
    use super::Face;
    pub const SANS: &[Face] = &[("Noto Sans:style=Regular", "Noto Sans"), ("DejaVu Sans:style=Book", "DejaVu Sans")];
    pub const SANS_SEMIBOLD: &[Face] = &[("Noto Sans:style=SemiBold", "Noto Sans"), ("DejaVu Sans:style=Bold", "DejaVu Sans")];
    pub const JA: &[Face] = &[("Noto Sans CJK JP:style=Regular", "Noto Sans CJK JP"), ("Source Han Sans JP:style=Regular", "Source Han Sans JP")];
    pub const JA_SEMIBOLD: &[Face] = &[("Noto Sans CJK JP:style=Bold", "Noto Sans CJK JP"), ("Source Han Sans JP:style=Bold", "Source Han Sans JP")];
    pub const KO: &[Face] = &[("Noto Sans CJK KR:style=Regular", "Noto Sans CJK KR"), ("Source Han Sans KR:style=Regular", "Source Han Sans KR")];
    pub const KO_SEMIBOLD: &[Face] = &[("Noto Sans CJK KR:style=Bold", "Noto Sans CJK KR"), ("Source Han Sans KR:style=Bold", "Source Han Sans KR")];
    pub const ZH_HANS: &[Face] = &[("Noto Sans CJK SC:style=Regular", "Noto Sans CJK SC"), ("Source Han Sans SC:style=Regular", "Source Han Sans SC")];
    pub const ZH_HANS_SEMIBOLD: &[Face] = &[("Noto Sans CJK SC:style=Bold", "Noto Sans CJK SC"), ("Source Han Sans SC:style=Bold", "Source Han Sans SC")];
    pub const ZH_HANT: &[Face] = &[("Noto Sans CJK TC:style=Regular", "Noto Sans CJK TC"), ("Source Han Sans TC:style=Regular", "Source Han Sans TC")];
    pub const ZH_HANT_SEMIBOLD: &[Face] = &[("Noto Sans CJK TC:style=Bold", "Noto Sans CJK TC"), ("Source Han Sans TC:style=Bold", "Source Han Sans TC")];
    pub const INSTALL: [&str; 2] = ["Noto Sans or DejaVu Sans (fonts-noto-core, fonts-dejavu-core).", "Noto Sans CJK (fonts-noto-cjk, noto-fonts-cjk, google-noto-sans-cjk-fonts)."];
}

#[cfg(windows)]
fn load(&(file, family, style): &Face) -> Option<FontData> {
    use skrifa::string::StringId;
    use skrifa::MetadataProvider;
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let bytes = std::fs::read(std::path::Path::new(&windows).join("Fonts").join(file)).ok()?;
    let name = |font: &skrifa::FontRef, id| font.localized_strings(id).english_or_first().map(|s| s.to_string());
    let index = skrifa::raw::FileRef::new(&bytes)
        .ok()?
        .fonts()
        .position(|font| {
            font.is_ok_and(|font| {
                name(&font, StringId::FAMILY_NAME).as_deref() == Some(family) && name(&font, StringId::SUBFAMILY_NAME).as_deref() == Some(style)
            })
        })?;
    face_data(bytes, u32::try_from(index).ok()?)
}

#[cfg(target_os = "linux")]
fn load(&(pattern, family): &Face) -> Option<FontData> {
    let out = fc_match(pattern)?;
    let mut lines = out.lines();
    let (file, index, families) = (lines.next()?, lines.next()?, lines.next()?);
    if !families.split(',').any(|f| f == family) {
        return None;
    }
    face_data(std::fs::read(file).ok()?, index.parse().ok()?)
}

/// fc-match's answer for `pattern`, or None when it fails or takes over two seconds (a cold
/// font cache on a slow disk): the language then shows without its face rather than waiting.
#[cfg(target_os = "linux")]
fn fc_match(pattern: &str) -> Option<String> {
    use std::io::Read;
    let mut child = crate::amd_decode::for_child(&mut std::process::Command::new("fc-match"))
        .args(["--format=%{file}\n%{index}\n%{family}\n", pattern])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(None) => {
                eprintln!("nightjar: fc-match did not answer for {pattern:?} within 2 s");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            _ => return None,
        }
    }
    // Three short lines: they fit the pipe, so the exit did not wait on this read.
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// `bytes` as egui's font, or None when egui could not parse face `index` (it panics on one).
/// fontconfig gives a variable font's named instance in the upper 16 bits of the index,
/// `(instance + 1) << 16 | face`: the instance becomes the face's axis coordinates.
fn face_data(bytes: Vec<u8>, index: u32) -> Option<FontData> {
    use skrifa::MetadataProvider;
    let (face, instance) = (index & 0xFFFF, index >> 16);
    let font = skrifa::FontRef::from_index(&bytes, face).ok()?;
    let coords = instance.checked_sub(1).and_then(|i| font.named_instances().get(usize::try_from(i).ok()?)).map(|named| {
        egui::epaint::text::VariationCoords::new(font.axes().iter().map(|axis| axis.tag()).zip(named.user_coords()))
    });
    let mut data = FontData::from_owned(bytes);
    data.index = face;
    if let Some(coords) = coords {
        data.tweak.coords = coords;
    }
    Some(data)
}

/// Sets the palette and the style that goes with it: high contrast or the theme's look as it was.
/// `still` turns egui's animations off (reduced motion): focus and hover fades, collapsing and
/// scrolling jump instead of glide.
pub fn set_look(ctx: &egui::Context, high: bool, still: bool) {
    set_high_contrast(high);
    STILL.store(still, Ordering::Relaxed);
    apply_style(ctx, look(), still);
}

/// Sets the look ([`Look::of`] the Theme setting) and the style that goes with it. A change of
/// look is a cut, never a fade, so it needs nothing for reduced motion.
pub fn set_theme_look(ctx: &egui::Context, look: Look, still: bool) {
    set_high_contrast(look == Look::HighContrast);
    if look != Look::HighContrast {
        set_base_look(look);
    }
    STILL.store(still, Ordering::Relaxed);
    apply_style(ctx, self::look(), still);
}

/// egui's style for `look`, which the roles already have.
fn apply_style(ctx: &egui::Context, look: Look, still: bool) {
    let high = look == Look::HighContrast;
    let day = look == Look::Day;
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, semibold(text::TITLE)),
        (TextStyle::Body, regular(text::BODY)),
        (TextStyle::Button, semibold(text::LABEL)),
        (TextStyle::Small, regular(text::CAPTION)),
        (TextStyle::Monospace, FontId::new(text::CAPTION, FontFamily::Monospace)),
    ]
    .into();
    // Within a group; pages put `space::L` between groups.
    style.spacing.item_spacing = egui::vec2(space::S, space::S);
    style.spacing.button_padding = egui::vec2(space::L, 9.0);
    style.spacing.interact_size.y = 34.0;
    style.spacing.combo_width = 220.0;
    style.animation_time = if still { 0.0 } else { egui::Style::default().animation_time };
    style.scroll_animation = if still { egui::style::ScrollAnimation::none() } else { egui::style::ScrollAnimation::default() };
    let v = &mut style.visuals;
    // egui's own defaults (text cursor, scroll bars, code) for a light page by day.
    *v = if day { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = *NIGHT;
    // By day a window is white over the pale page, Android's dialogs on `surface_container_lowest`;
    // on OLED black it is black with its outline, as Android's `outlineOnBlack`.
    v.window_fill = match look {
        Look::Day | Look::Oled => *SURFACE_LOWEST,
        Look::Night | Look::HighContrast => *SURFACE_HIGH,
    };
    v.window_stroke = Stroke::new(if high { 2.0 } else { 1.0 }, *OUTLINE_VARIANT);
    v.extreme_bg_color = *SURFACE_LOWEST;
    v.faint_bg_color = *SURFACE_LOW;
    v.override_text_color = Some(*ON_SURFACE);
    // egui's own weak text (half-faded) and disabled controls would fall below the contrast of
    // the rest; weak text is the secondary text role, and disabled fades less.
    v.weak_text_color = Some(*ON_SURFACE_VARIANT);
    v.disabled_alpha = if high { 0.75 } else { 0.62 };
    v.hyperlink_color = *MOON;
    v.selection.bg_fill = *MOON_CONTAINER;
    v.selection.stroke = Stroke::new(1.0, *ON_MOON_CONTAINER);
    v.window_corner_radius = radius::PANEL.into();
    v.menu_corner_radius = radius::CONTROL.into();
    v.window_shadow = egui::Shadow::NONE;
    // Menus float over the page: a soft shadow lifts them off it (none in high contrast, where
    // their outline does it); a lighter one by day, as the day glass's.
    v.popup_shadow = if high {
        egui::Shadow::NONE
    } else {
        egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(if day { 46 } else { 120 }) }
    };
    let radius = egui::CornerRadius::same(radius::CONTROL);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, *SURFACE),
        (&mut v.widgets.inactive, *SURFACE_HIGH),
        (&mut v.widgets.hovered, *SURFACE_HIGHEST),
        (&mut v.widgets.active, *SURFACE_HIGHEST),
        (&mut v.widgets.open, *SURFACE_HIGHEST),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = radius;
        w.fg_stroke = Stroke::new(1.0, *ON_SURFACE);
        // High contrast outlines every control, so a button reads as one without its fill.
        w.bg_stroke = if high { Stroke::new(1.5, *OUTLINE_VARIANT) } else { Stroke::NONE };
        w.expansion = 0.0;
    }
    // A check box or radio draws its box in `bg_fill` (buttons use `weak_bg_fill`): on a window,
    // whose fill is SURFACE_HIGH too, an unchecked box would vanish. High contrast outlines it.
    if !high {
        v.widgets.inactive.bg_fill = *OUTLINE_VARIANT;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, *OUTLINE_VARIANT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, *ON_SURFACE_VARIANT);
    // Hover lightens the fill one step and draws no outline (high contrast keeps the outline every
    // control has). Focus, which egui draws with the active look, is the amber ring.
    v.widgets.hovered.bg_stroke = if high { Stroke::new(1.5, *OUTLINE_VARIANT) } else { Stroke::NONE };
    v.widgets.active.bg_stroke = Stroke::new(focus_ring_width(), *MOON);
    ctx.set_global_style(style);
}

/// The brand mark: the nightjar crossing the moon, from `site/assets/og.svg` (a 108-unit
/// adaptive icon: moon radius 27 around (54, 54)).
const BIRD: &str = "M 51.75,41.24 C 53.31,40.96 54.50,41.92 54.89,43.31 C 55.16,44.00 55.33,44.55 55.55,44.95 C 58.63,42.95 61.40,40.41 64.45,38.63 C 68.66,39.72 73.12,42.22 78.13,45.72 C 72.88,44.97 68.19,45.36 64.07,46.82 C 60.97,47.94 58.40,49.93 56.11,51.87 L 56.49,56.49 L 59.86,66.49 C 58.86,67.47 57.71,67.97 56.50,68.18 C 55.30,68.39 54.04,68.32 52.77,67.74 L 52.51,57.19 L 51.29,52.72 C 48.47,51.68 45.38,50.70 42.08,50.69 C 37.70,50.73 33.17,51.97 28.50,54.48 C 32.00,49.47 35.34,45.59 38.92,43.13 C 42.40,43.76 45.87,45.20 49.45,46.03 C 49.52,45.58 49.49,45.00 49.51,44.26 C 49.40,42.82 50.19,41.51 51.75,41.24 Z";

/// The bird alone over the moon's box (27 to 81 units), white with coverage as its alpha, to tint.
pub fn bird_mask(size: usize) -> egui::ColorImage {
    const SUB: usize = 4;
    const LEFT: f32 = 27.0;
    const SPAN: f32 = 54.0;
    let polygon = bird_polygon();
    let mut coverage = vec![0.0f32; size * size];
    let mut crossings = Vec::new();
    for row in 0..size * SUB {
        let y = LEFT + (row as f32 + 0.5) / (size * SUB) as f32 * SPAN;
        crossings.clear();
        for (i, a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            if (a.1 > y) != (b.1 > y) {
                let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
                crossings.push((x - LEFT) / SPAN * size as f32);
            }
        }
        crossings.sort_by(f32::total_cmp);
        for span in crossings.as_chunks::<2>().0 {
            let (from, to) = (span[0].clamp(0.0, size as f32), span[1].clamp(0.0, size as f32));
            let line = row / SUB;
            for x in from as usize..(to.ceil() as usize).min(size) {
                let over = (to.min(x as f32 + 1.0) - from.max(x as f32)).max(0.0);
                coverage[line * size + x] += over / SUB as f32;
            }
        }
    }
    let pixels = coverage
        .iter()
        .map(|c| egui::Color32::from_white_alpha((c.min(1.0) * 255.0).round() as u8))
        .collect();
    egui::ColorImage {
        size: [size, size],
        source_size: egui::vec2(size as f32, size as f32),
        pixels,
    }
}

/// The bird outline flattened to a polygon in icon units.
fn bird_polygon() -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut tokens = BIRD
        .split([' ', ','])
        .filter(|t| !t.is_empty())
        .peekable();
    let mut current = (0.0f32, 0.0f32);
    let mut command = 'M';
    let number = |tokens: &mut std::iter::Peekable<_>| -> f32 {
        let t: &str = tokens.next().unwrap_or("0");
        t.parse().unwrap_or(0.0)
    };
    while let Some(&token) = tokens.peek() {
        if let Some(c) = token.chars().next().filter(char::is_ascii_alphabetic) {
            command = c;
            tokens.next();
            if c == 'Z' {
                break;
            }
            continue;
        }
        match command {
            'M' | 'L' => {
                current = (number(&mut tokens), number(&mut tokens));
                points.push(current);
            }
            'C' => {
                let c1 = (number(&mut tokens), number(&mut tokens));
                let c2 = (number(&mut tokens), number(&mut tokens));
                let end = (number(&mut tokens), number(&mut tokens));
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let x = u * u * u * current.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * end.0;
                    let y = u * u * u * current.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * end.1;
                    points.push((x, y));
                }
                current = end;
            }
            _ => {
                tokens.next();
            }
        }
    }
    points
}

fn inside(polygon: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A number field a controller or the arrow keys can leave. egui's `DragValue` edits as soon as it has
/// focus and steps on Up and Down, so those keys changed the value and the user was stuck on the row.
/// Here Up and Down stay with focus navigation like every other row, and Left and Right step the value
/// by `step`. Once the user types into the field, Left and Right move the caret again until focus leaves.
pub fn step_value<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    style: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    let id = ui.next_auto_id();
    let typing = id.with("typing");
    let mut stepped = false;
    if ui.is_enabled() && ui.memory(|m| m.has_focus(id)) {
        let typed = ui.data(|d| d.get_temp::<bool>(typing).unwrap_or(false))
            || ui.input(|i| i.events.iter().any(|e| match e {
                egui::Event::Text(_) | egui::Event::Paste(_) => true,
                egui::Event::Key { key, pressed: true, .. } => matches!(key, egui::Key::Backspace | egui::Key::Delete),
                _ => false,
            }));
        if typed { ui.data_mut(|d| d.insert_temp(typing, true)); }
        let steps = ui.input_mut(|i| {
            // Focus has already moved on these (egui reads them before the frame); the field must not step too.
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            if typed { return 0; }
            i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) as i64
                - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) as i64
        });
        if steps != 0 {
            let (low, high) = (range.start().to_f64(), range.end().to_f64());
            let before = value.to_f64();
            *value = N::from_f64((before + steps as f64 * step).clamp(low, high));
            stepped = value.to_f64() != before;
        }
    } else {
        ui.data_mut(|d| d.remove::<bool>(typing));
    }
    let mut response = ui.add(style(egui::DragValue::new(value).range(range)));
    // The value changed before the DragValue saw it, so the response would not say so; callers that
    // write the value back on `changed()` (the stream menu's fps) must hear about the step.
    if stepped { response.mark_changed(); }
    if ui.is_enabled() && response.has_focus() {
        // Its text field has just locked every arrow key to the caret; give Up and Down back to focus.
        let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(id, filter));
    }
    response
}

/// A slider a controller or the arrow keys can leave. egui's slider shows its value in a `DragValue`
/// of its own, which steps on Up and Down once it has focus: Up from the row below landed there and
/// only raised the value (the stream menu's bitrate). Here the value is a [`step_value`] beside the
/// rail, so Up and Down move focus on both, and Left and Right step on both. While the rail is
/// dragged, and for [`VALUE_AFTER_KEY`] seconds after a key step, `shown` of the value floats over the
/// thumb, where a finger on a touch screen does not cover it (nightjar#61).
pub fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    slider_along(ui, value, range, step, false, shown, rail, field)
}

/// [`slider`] whose rail runs in equal ratios, for a value across orders of magnitude. The rail
/// has to be told so with `logarithmic(true)` in `rail`.
pub fn log_slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    slider_along(ui, value, range, step, true, shown, rail, field)
}

/// A whole percent as a slider's value shows it, `50%`.
pub fn percent(value: f64) -> String {
    format!("{value:.0}%")
}

/// A bitrate in Mbps as the settings' field shows it, `20.0 Mbps`.
pub fn mbps_text(mbps: f64) -> String {
    format!("{mbps:.1} Mbps")
}

/// How long a slider's value stays over its thumb after the last key step, in seconds; Android's
/// `StepSlider.KEY_VALUE_MS`.
pub const VALUE_AFTER_KEY: f64 = 1.5;

/// Whether a slider's value floats over its thumb: while it is dragged, and while it has the focus
/// within [`VALUE_AFTER_KEY`] of a key step `since_step` seconds ago.
fn value_floats(dragged: bool, focused: bool, since_step: Option<f64>) -> bool {
    dragged || (focused && since_step.is_some_and(|since| (0.0..VALUE_AFTER_KEY).contains(&since)))
}

/// Where the value floats: centred over the thumb at `thumb`, clear of it, inside `screen`.
fn value_rect(thumb: egui::Pos2, label: egui::Vec2, screen: egui::Rect) -> egui::Rect {
    let size = label + egui::vec2(2.0 * space::M, 2.0 * space::XS);
    let bottom = thumb.y - t::SIZE_SLIDER_THUMB / 2.0 - space::S;
    let rect = egui::Rect::from_min_size(egui::pos2(thumb.x - size.x / 2.0, bottom - size.y), size);
    let shift = (screen.left() - rect.left()).max(0.0) + (screen.right() - rect.right()).min(0.0);
    rect.translate(egui::vec2(shift, (screen.top() - rect.top()).max(0.0)))
}

/// The value over the thumb, in Android's slider label: the inverse surface, on top of everything.
fn paint_value(ui: &egui::Ui, thumb: egui::Pos2, shown: String) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("slider-value")));
    let galley = painter.layout_no_wrap(shown, FontId::proportional(text::LABEL), *SURFACE);
    let rect = value_rect(thumb, galley.size(), ui.ctx().content_rect());
    painter.rect_filled(rect, egui::CornerRadius::same(radius::SMALL), *ON_SURFACE);
    painter.galley(rect.center() - galley.size() / 2.0, galley, *SURFACE);
}

/// Where `value` stands along a rail, 0 to 1. Only a range above zero runs in ratios here.
fn rail_position(value: f64, low: f64, high: f64, ratios: bool) -> f32 {
    let along = if ratios && low > 0.0 {
        (value.max(low) / low).ln() / (high / low).ln()
    } else {
        (value - low) / (high - low)
    };
    if along.is_finite() { along.clamp(0.0, 1.0) as f32 } else { 0.0 }
}

#[allow(clippy::too_many_arguments)] // the slider's value, range, step and look, passed as egui takes them
fn slider_along<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    ratios: bool,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    ui.horizontal(|ui| {
        let (low, high) = (range.start().to_f64(), range.end().to_f64());
        // egui's slider keeps the input; the rail and thumb are ours, a moon fill and a paper
        // thumb, which egui's one colour for rail and thumb cannot draw.
        let slid = ui.scope(|ui| {
            ui.spacing_mut().slider_width = 150.0;
            let v = ui.visuals_mut();
            v.selection.bg_fill = Color32::TRANSPARENT;
            for look in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                look.bg_fill = Color32::TRANSPARENT;
                look.fg_stroke = Stroke::NONE;
                look.bg_stroke = Stroke::NONE;
            }
            ui.add(rail(egui::Slider::new(value, range.clone()).show_value(false)))
        }).inner;
        let typed = step_value(ui, value, range, step, field);
        let along = rail_position(value.to_f64(), low, high, ratios);
        let thumb = paint_rail(ui, &slid, along);
        let stepped_at = slid.id.with("stepped-at");
        let now = ui.input(|i| i.time);
        if (slid.changed() || typed.changed()) && slid.has_focus() && !slid.dragged() {
            ui.data_mut(|d| d.insert_temp(stepped_at, now));
        }
        let since_step = ui.data(|d| d.get_temp::<f64>(stepped_at)).map(|at| now - at);
        if value_floats(slid.dragged(), slid.has_focus(), since_step) {
            paint_value(ui, thumb, shown(value.to_f64()));
            if !slid.dragged() {
                ui.ctx().request_repaint_after_secs((VALUE_AFTER_KEY - since_step.unwrap_or(0.0)) as f32);
            }
        }
        slid | typed
    })
    .inner
}

/// The rail and its thumb; answers where the thumb's centre is.
fn paint_rail(ui: &egui::Ui, slid: &egui::Response, along: f32) -> egui::Pos2 {
    use crate::ui::design_tokens as t;
    let rect = slid.rect;
    // Where egui's slider has its thumb, so a drag stays under the pointer.
    let reach = (rect.height() / 2.5).round();
    let thumb_x = egui::lerp(rect.left() + reach..=rect.right() - reach, along);
    let middle = rect.center().y;
    let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), middle - t::SIZE_SLIDER_TRACK / 2.0), egui::pos2(rect.right(), middle + t::SIZE_SLIDER_TRACK / 2.0));
    let round = egui::CornerRadius::same((t::SIZE_SLIDER_TRACK / 2.0) as u8);
    let high = high_contrast();
    let painter = ui.painter();
    painter.rect_filled(bar, round, if high { *SURFACE_HIGHEST } else { shade::TINT_TRACK_FILL.get() });
    if high {
        painter.rect_stroke(bar, round, Stroke::new(1.0, t::HC_COLOR_RIM), egui::StrokeKind::Outside);
    }
    let filled = egui::Rect::from_min_max(bar.min, egui::pos2(thumb_x, bar.bottom()));
    painter.rect_filled(filled, round, *MOON);
    let center = egui::pos2(thumb_x, middle);
    painter.circle_filled(center + egui::vec2(0.0, 1.0), t::SIZE_SLIDER_THUMB / 2.0, Color32::from_black_alpha(70));
    painter.circle_filled(center, t::SIZE_SLIDER_THUMB / 2.0, knob());
    if slid.has_focus() {
        let spot = egui::Rect::from_center_size(center, egui::Vec2::splat(t::SIZE_SLIDER_THUMB));
        crate::ui::focus_ring_around(ui, spot, (t::SIZE_SLIDER_THUMB / 2.0) as u8);
    }
    center
}

/// The mark as straight RGBA pixels, `size` square: the amber moon with the night bird cut
/// out, 4x4 supersampled.
pub fn logo_rgba(size: usize) -> Vec<u8> {
    let bird = bird_polygon();
    // The mark keeps the brand colours in every palette.
    let (brand_moon, brand_night) = (NORMAL[Index::MOON as usize], NORMAL[Index::NIGHT as usize]);
    let mut out = vec![0u8; size * size * 4];
    let scale = 54.0 / size as f32; // the moon fills the image: 27 units of radius
    for py in 0..size {
        for px in 0..size {
            let (mut moon, mut cut) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = 27.0 + (px as f32 + (sx as f32 + 0.5) / 4.0) * scale;
                    let y = 27.0 + (py as f32 + (sy as f32 + 0.5) / 4.0) * scale;
                    let in_moon = (x - 54.0).powi(2) + (y - 54.0).powi(2) <= 27.0 * 27.0;
                    if in_moon {
                        moon += 1;
                        if inside(&bird, x, y) {
                            cut += 1;
                        }
                    }
                }
            }
            let i = (py * size + px) * 4;
            if moon == 0 {
                continue;
            }
            // Bird pixels are the night colour; moon pixels amber; edges blend.
            let amber = (moon - cut) as f32;
            let night = cut as f32;
            let total = amber + night;
            let mix = |a: u8, n: u8| ((f32::from(a) * amber + f32::from(n) * night) / total) as u8;
            out[i] = mix(brand_moon.r(), brand_night.r());
            out[i + 1] = mix(brand_moon.g(), brand_night.g());
            out[i + 2] = mix(brand_moon.b(), brand_night.b());
            out[i + 3] = (moon * 255 / 16) as u8;
        }
    }
    out
}

pub fn logo_image(size: usize) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([size, size], &logo_rgba(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The moon fill and thumb stand where egui's own hit area does: equal ratios for bitrate.
    #[test]
    fn a_rail_places_its_thumb_by_the_scale_it_runs_in() {
        assert_eq!(rail_position(150.0, 100.0, 200.0, false), 0.5);
        assert!((rail_position(1000.0, 100.0, 10_000.0, true) - 0.5).abs() < 1e-5);
        assert_eq!(rail_position(50.0, 100.0, 200.0, true), 0.0);
        assert_eq!(rail_position(900.0, 100.0, 200.0, false), 1.0);
        // A range from zero cannot run in ratios, and an empty one has nowhere to go.
        assert_eq!(rail_position(5.0, 0.0, 10.0, true), 0.5);
        assert_eq!(rail_position(5.0, 5.0, 5.0, false), 0.0);
    }

    /// nightjar#61: the value floats over the thumb while it moves, then goes.
    #[test]
    fn a_slider_shows_its_value_while_it_moves() {
        assert!(value_floats(true, false, None), "a drag shows it");
        assert!(value_floats(false, true, Some(0.0)), "a key step shows it");
        assert!(value_floats(false, true, Some(VALUE_AFTER_KEY - 0.1)));
        assert!(!value_floats(false, true, Some(VALUE_AFTER_KEY)), "and it goes after a while");
        assert!(!value_floats(false, false, Some(0.1)), "or once the focus leaves");
        assert!(!value_floats(false, true, None), "focus alone does not show it");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let rect = value_rect(egui::pos2(200.0, 100.0), egui::vec2(60.0, 14.0), screen);
        assert!(rect.bottom() < 100.0 - t::SIZE_SLIDER_THUMB / 2.0, "above the thumb, clear of the finger on it");
        assert_eq!(rect.center().x, 200.0);
        let edge = value_rect(egui::pos2(395.0, 100.0), egui::vec2(60.0, 14.0), screen);
        assert!(screen.contains_rect(edge), "a thumb at the edge keeps its value on screen");
        assert_eq!(percent(55.0), "55%");
        assert_eq!(mbps_text(20.0), "20.0 Mbps");
        assert_eq!(mbps_text(2.5), "2.5 Mbps");
    }

    /// The text a headless frame with one slider at `value` draws, after `events`.
    fn slider_frame(ctx: &egui::Context, value: &mut f64, time: f64, events: Vec<egui::Event>) -> (Vec<String>, egui::Rect) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut rail = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(input, |ui| {
            ui.add_space(60.0);
            rail = slider(ui, value, 0.0..=100.0, 1.0, |v| format!("{v:.0} units"), |rail| rail, |field| field).rect;
        });
        output.textures_delta.clear();
        let mut drawn = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape { drawn.push(text.galley.text().to_owned()); }
        }
        (drawn, rail)
    }

    #[test]
    fn dragging_a_slider_floats_its_value_and_letting_go_hides_it() {
        let ctx = egui::Context::default();
        let mut value = 20.0;
        let _ = slider_frame(&ctx, &mut value, 1.0, Vec::new());
        let (drawn, rail) = slider_frame(&ctx, &mut value, 1.1, Vec::new());
        assert!(!drawn.iter().any(|t| t.ends_with("units")), "at rest only the field shows it: {drawn:?}");
        let start = egui::pos2(rail.left() + 30.0, rail.center().y);
        let held = egui::pos2(rail.left() + 90.0, rail.center().y);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        slider_frame(&ctx, &mut value, 1.2, vec![egui::Event::PointerMoved(start), button(start, true)]);
        let (drawn, _) = slider_frame(&ctx, &mut value, 1.3, vec![egui::Event::PointerMoved(held)]);
        let shown = format!("{value:.0} units");
        assert!(drawn.contains(&shown), "the value floats while dragged: {shown} in {drawn:?}");
        slider_frame(&ctx, &mut value, 1.4, vec![button(held, false)]);
        let (drawn, _) = slider_frame(&ctx, &mut value, 1.5, Vec::new());
        assert!(!drawn.contains(&shown), "and goes when let go: {drawn:?}");
    }

    #[test]
    fn bird_parses_into_a_closed_shape_inside_the_moon() {
        let bird = bird_polygon();
        assert!(bird.len() > 100);
        assert!(bird.iter().all(|&(x, y)| (27.0..=81.0).contains(&x) && (27.0..=81.0).contains(&y)));
        // The body is solid, the sky above the wings is not.
        assert!(inside(&bird, 54.5, 60.0));
        assert!(!inside(&bird, 54.0, 32.0));
    }

    /// A button, a stepped number field and a button in one column, in a headless egui context.
    struct Column {
        ctx: egui::Context,
        value: u32,
        time: f64,
        ids: [egui::Id; 3],
        /// Whether the field reported a change in the frame that carried the last key.
        changed: bool,
    }

    impl Column {
        fn new(value: u32) -> Self {
            let mut column = Self { ctx: egui::Context::default(), value, time: 1.0, ids: [egui::Id::NULL; 3], changed: false };
            column.frame(Vec::new());
            column.frame(Vec::new());
            column
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> bool {
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let (value, ids) = (&mut self.value, &mut self.ids);
            let mut changed = false;
            let mut output = self.ctx.run_ui(input, |ui| {
                ids[0] = ui.button("above").id;
                let field = step_value(ui, value, 1000..=5000, 500.0, |d| d.suffix(" kbps"));
                (ids[1], changed) = (field.id, field.changed());
                ids[2] = ui.button("below").id;
            });
            output.textures_delta.clear();
            changed
        }

        fn press(&mut self, key: egui::Key) {
            self.changed = self.frame(crate::gui::tap(key).to_vec());
            self.frame(Vec::new());
        }

        fn focus_field(&mut self) {
            let field = self.ids[1];
            self.ctx.memory_mut(|m| m.request_focus(field));
            self.frame(Vec::new());
            self.frame(Vec::new());
            assert_eq!(self.ctx.memory(|m| m.focused()), Some(field));
        }

        fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|m| m.focused())
        }
    }

    #[test]
    fn up_and_down_leave_a_number_field_without_changing_it() {
        let mut column = Column::new(3000);
        column.focus_field();
        column.press(egui::Key::ArrowDown);
        assert_eq!(column.focused(), Some(column.ids[2]), "Down moves to the row below");
        assert_eq!(column.value, 3000, "Down does not step the field");
        column.focus_field();
        column.press(egui::Key::ArrowUp);
        assert_eq!(column.focused(), Some(column.ids[0]), "Up moves to the row above");
        assert_eq!(column.value, 3000, "Up does not step the field");
    }

    #[test]
    fn left_and_right_step_a_number_field_inside_its_range() {
        let mut column = Column::new(4500);
        column.focus_field();
        column.press(egui::Key::ArrowRight);
        assert_eq!(column.value, 5000);
        assert!(column.changed, "a step is reported as a change, so callers write it back");
        column.press(egui::Key::ArrowRight);
        assert_eq!(column.value, 5000, "the top of the range holds");
        assert!(!column.changed, "holding at the top of the range is no change");
        assert_eq!(column.focused(), Some(column.ids[1]), "Right keeps the field focused");
        column.press(egui::Key::ArrowLeft);
        column.press(egui::Key::ArrowLeft);
        assert_eq!(column.value, 4000);
        assert_eq!(column.focused(), Some(column.ids[1]), "Left keeps the field focused");
        for _ in 0..10 { column.press(egui::Key::ArrowLeft); }
        assert_eq!(column.value, 1000, "the bottom of the range holds");
    }

    /// Typing into the field gives Left and Right back to the caret, so they neither step the value
    /// nor clobber the typed text; once focus leaves and comes back they step again.
    #[test]
    fn left_and_right_move_the_caret_once_the_user_types() {
        let mut column = Column::new(2000);
        column.focus_field();
        column.frame(vec![egui::Event::Text("5".into())]);
        column.frame(Vec::new());
        let typed = column.value;
        assert!(typed >= 2000, "room for Left to step: {typed}");
        column.press(egui::Key::ArrowLeft);
        column.press(egui::Key::ArrowLeft);
        assert_eq!(column.value, typed, "Left does not step while typing");
        assert_eq!(column.focused(), Some(column.ids[1]));
        column.press(egui::Key::ArrowDown);
        assert_eq!(column.focused(), Some(column.ids[2]), "Down still leaves the field");
        column.focus_field();
        let before = column.value;
        column.press(egui::Key::ArrowLeft);
        assert_eq!(column.value, (before - 500).max(1000), "back on the field, Left steps again");
    }

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// WCAG contrast ratio of two colours, 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn color(look: Look, role: Role) -> Color32 {
        palette(look)[usize::from(role.0)]
    }

    const GROUNDS: [Role; 4] = [NIGHT, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST];

    /// Every look and the contrast its text needs: WCAG AA, 7:1 in high contrast.
    const LOOKS: [(Look, f64); 4] = [(Look::Night, 4.5), (Look::Day, 4.5), (Look::Oled, 4.5), (Look::HighContrast, 7.0)];

    #[test]
    fn text_meets_wcag_aa_in_every_look_and_seven_to_one_in_high_contrast() {
        for (look, floor) in LOOKS {
            for text in [ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE, MOON, TERTIARY, ERROR, OK, WARN] {
                for ground in GROUNDS {
                    let ratio = contrast(color(look, text), color(look, ground));
                    assert!(ratio >= floor, "{look:?}: text {text:?} on {ground:?} is {ratio:.2}:1, wants {floor}");
                }
            }
            let on_moon = contrast(color(look, ON_MOON), color(look, MOON));
            assert!(on_moon >= floor, "{look:?}: button text on the moon is {on_moon:.2}:1");
            let on_container = contrast(color(look, ON_MOON_CONTAINER), color(look, MOON_CONTAINER));
            assert!(on_container >= floor, "{look:?}: text on a selection is {on_container:.2}:1");
            let on_error = contrast(color(look, ON_ERROR_CONTAINER), color(look, ERROR_CONTAINER));
            assert!(on_error >= floor, "{look:?}: text on a failure is {on_error:.2}:1");
        }
    }

    #[test]
    fn the_theme_picks_the_look_and_high_contrast_wins() {
        use crate::config::Theme;
        for system_light in [false, true] {
            assert_eq!(Look::of(Theme::Dark, system_light, false), Look::Night, "Dark whatever the system says");
            assert_eq!(Look::of(Theme::Oled, system_light, false), Look::Oled, "OLED black whatever the system says");
            for theme in Theme::ALL {
                assert_eq!(Look::of(theme, system_light, true), Look::HighContrast, "{theme:?}: high contrast wins");
            }
        }
        assert_eq!(Look::of(Theme::System, false, false), Look::Night, "System on a dark system");
        assert_eq!(Look::of(Theme::System, true, false), Look::Day, "System on a light system");
        assert_eq!(Look::of(Theme::default(), false, false), Look::Night, "Dark is the default");
    }

    #[test]
    fn the_theme_look_sets_the_roles_and_egui_light_or_dark() {
        let ctx = egui::Context::default();
        own_palette();
        for look in [Look::Day, Look::Oled, Look::HighContrast, Look::Night] {
            set_theme_look(&ctx, look, false);
            assert_eq!(self::look(), look);
            assert_eq!(*ON_SURFACE, color(look, ON_SURFACE), "{look:?}");
            let v = ctx.global_style().visuals.clone();
            assert_eq!(v.dark_mode, look != Look::Day, "{look:?}: egui's own widgets match the page");
            assert_eq!(v.panel_fill, color(look, NIGHT), "{look:?}");
        }
        // High contrast keeps the theme's look under it, and turning it off brings that back.
        set_theme_look(&ctx, Look::Day, false);
        set_look(&ctx, true, false);
        assert_eq!(look(), Look::HighContrast);
        set_look(&ctx, false, false);
        assert_eq!(look(), Look::Day);
        assert!(!ctx.global_style().visuals.dark_mode);
    }

    #[test]
    fn over_video_draws_the_night_look_and_keeps_high_contrast() {
        let ctx = egui::Context::default();
        own_palette();
        for base in [Look::Day, Look::Oled] {
            set_theme_look(&ctx, base, false);
            set_over_video(&ctx, true);
            assert_eq!(look(), Look::Night, "{base:?}: the stream menu and overlay keep the night look");
            assert_eq!(*SURFACE, color(Look::Night, SURFACE));
            assert!(ctx.global_style().visuals.dark_mode, "{base:?}: egui's style follows");
            set_over_video(&ctx, false);
            assert_eq!(look(), base, "the launcher's look comes back with its frames");
            assert_eq!(ctx.global_style().visuals.panel_fill, color(base, NIGHT));
        }
        set_theme_look(&ctx, Look::HighContrast, false);
        set_over_video(&ctx, true);
        assert_eq!(look(), Look::HighContrast, "high contrast over video too");
        set_over_video(&ctx, false);
    }

    #[test]
    fn oled_is_the_night_palette_on_black() {
        let surfaces = [NIGHT, SURFACE_LOWEST, SURFACE_LOW, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST, SURFACE_BRIGHT].map(|role| usize::from(role.0));
        for i in (0..COUNT).filter(|i| !surfaces.contains(i)) {
            assert_eq!(OLED[i], NORMAL[i], "role {i} is the night one on OLED black");
        }
        for role in [NIGHT, SURFACE_LOWEST, SURFACE_LOW, SURFACE] {
            assert_eq!(color(Look::Oled, role), Color32::BLACK, "{role:?}");
        }
        // The steps a hover and a press climb to stay apart from black and from each other.
        assert_ne!(color(Look::Oled, SURFACE_HIGH), Color32::BLACK);
        assert_ne!(color(Look::Oled, SURFACE_HIGH), color(Look::Oled, SURFACE_HIGHEST));
    }

    #[test]
    fn by_day_every_role_has_its_own_colour_and_text_is_dark_on_light() {
        for i in 0..COUNT {
            assert_ne!(DAY[i], NORMAL[i], "role {i} has a day colour of its own");
        }
        for ground in [NIGHT, SURFACE_LOWEST, SURFACE, SURFACE_HIGHEST] {
            for text in [ON_SURFACE, ON_SURFACE_VARIANT, MOON] {
                assert!(luminance(color(Look::Day, text)) < luminance(color(Look::Day, ground)), "{text:?} on {ground:?}");
            }
        }
    }

    #[test]
    fn a_shade_gives_the_value_of_the_look() {
        let (night, day, oled) = (Color32::RED, Color32::GREEN, Color32::BLUE);
        let three = Shade::with_oled(night, day, oled);
        assert_eq!([Look::Night, Look::Day, Look::Oled, Look::HighContrast].map(|look| three.of(look)), [night, day, oled, night]);
        assert_eq!(Shade::new(night, day).of(Look::Oled), night, "OLED takes the night value unless it has one");
        own_look(Look::Day);
        assert_eq!(three.get(), day);
        assert_eq!(knob(), Color32::WHITE, "a switch's thumb stays white by day");
        own_look(Look::Oled);
        assert_eq!(three.get(), oled);
        assert_eq!(shade::COLOR_CANVAS.get(), Color32::BLACK);
        own_look(Look::HighContrast);
        assert_eq!(three.get(), night, "high contrast draws the night value where it has no look of its own");
        own_palette();
        assert_eq!(knob(), color(Look::Night, PAPER));
    }

    #[test]
    fn status_on_the_scrim_reads_over_a_white_frame() {
        // Text over game art sits on the scrim; the worst frame behind it is white.
        let ground = over(SCRIM, Color32::WHITE);
        let white = contrast(Color32::WHITE, ground);
        assert!(white >= 7.0, "white on the scrim is {white:.2}:1");
        // By day the dot over art keeps the night green (`cards::running_dot`); the day one is for
        // light pages.
        for look in [Look::Night, Look::Oled, Look::HighContrast] {
            let ok = contrast(color(look, OK), ground);
            assert!(ok >= 4.5, "{look:?}: the running dot on the scrim is {ok:.2}:1");
        }
    }

    #[test]
    fn warn_stays_apart_from_the_amber_of_the_action() {
        // They are close (the rule keeps warn out of filled shapes), but never the same colour.
        // Yellower is more green for its red; by day both are dark inks.
        let yellow = |c: Color32| f32::from(c.g()) / f32::from(c.r()).max(1.0);
        for (look, _) in LOOKS {
            assert_ne!(color(look, WARN), color(look, MOON), "{look:?}");
            assert!(yellow(color(look, WARN)) > yellow(color(look, MOON)), "{look:?}: warn is the yellower one");
        }
    }

    #[test]
    fn the_look_follows_the_console_type_space_and_shape() {
        let ctx = egui::Context::default();
        own_palette();
        set_look(&ctx, false, false);
        let style = ctx.global_style();
        let size = |s: TextStyle| style.text_styles[&s].size;
        assert_eq!(
            [size(TextStyle::Heading), size(TextStyle::Body), size(TextStyle::Button), size(TextStyle::Small), size(TextStyle::Monospace)],
            [26.0, 16.0, 14.0, 12.0, 12.0]
        );
        assert_eq!(style.spacing.item_spacing, egui::vec2(8.0, 8.0));
        assert_eq!(style.spacing.button_padding.x, 16.0);
        assert_eq!(style.visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(16));
        assert_eq!(style.visuals.window_corner_radius, egui::CornerRadius::same(22));
        assert_eq!(style.visuals.menu_corner_radius, egui::CornerRadius::same(16));
        assert_eq!([space::XS, space::S, space::M, space::L, space::XL, space::XXL, space::XXXL], [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0]);
        assert_eq!(CONTROL_HEIGHT, 44.0);
    }

    #[test]
    fn an_unchecked_box_shows_on_a_window_and_on_a_page() {
        let ctx = egui::Context::default();
        own_palette();
        for (look, _) in LOOKS {
            set_theme_look(&ctx, look, false);
            let v = ctx.global_style().visuals.clone();
            let box_ = &v.widgets.inactive;
            if look == Look::HighContrast {
                assert!(box_.bg_stroke.width > 0.0, "high contrast outlines the box");
            } else {
                for ground in [v.window_fill, v.panel_fill] {
                    let ratio = contrast(box_.bg_fill, ground);
                    assert!(ratio >= 1.3, "{look:?}: box on {ground:?} is {ratio:.2}:1");
                }
                // Buttons keep their own fill.
                assert_eq!(box_.weak_bg_fill, color(look, SURFACE_HIGH), "{look:?}");
            }
        }
    }

    /// `top` (maybe translucent) painted over the opaque `ground`, as egui blends it.
    fn over(top: Color32, ground: Color32) -> Color32 {
        let keep = u32::from(255 - top.a());
        let mix = |t: u8, g: u8| (u32::from(t) + u32::from(g) * keep / 255).min(255) as u8;
        Color32::from_rgb(mix(top.r(), ground.r()), mix(top.g(), ground.g()), mix(top.b(), ground.b()))
    }

    #[test]
    fn weak_and_disabled_text_stay_readable_in_both_palettes() {
        // What `ui.weak` and a disabled control draw come from the style, not from a role of
        // ours, so read them from the style the app sets. Disabled text is exempt from WCAG; it
        // still has to be read, so it gets 3:1 (4.5:1 in high contrast).
        for (look, weak_floor) in LOOKS {
            let disabled_floor = if look == Look::HighContrast { 4.5 } else { 3.0 };
            let ctx = egui::Context::default();
            own_palette();
            set_theme_look(&ctx, look, false);
            let visuals = ctx.global_style().visuals.clone();
            for ground in GROUNDS {
                let ground = color(look, ground);
                let weak = over(visuals.weak_text_color(), ground);
                let ratio = contrast(weak, ground);
                assert!(ratio >= weak_floor, "{look:?}: weak text on {ground:?} is {ratio:.2}:1, wants {weak_floor}");
                let disabled = over(visuals.disable(visuals.text_color()), ground);
                let ratio = contrast(disabled, ground);
                assert!(ratio >= disabled_floor, "{look:?}: disabled text on {ground:?} is {ratio:.2}:1, wants {disabled_floor}");
            }
        }
    }

    #[test]
    fn the_focus_ring_stands_out_from_every_surface_in_every_look() {
        for (look, _) in LOOKS {
            for ground in GROUNDS {
                let ratio = contrast(color(look, MOON), color(look, ground));
                assert!(ratio >= 3.0, "{look:?}: ring on {ground:?} is {ratio:.2}:1, wants 3");
            }
        }
        // The ring is also thicker in high contrast.
        TEST_HIGH.with(|h| h.set(Some(false)));
        let normal = focus_ring_width();
        TEST_HIGH.with(|h| h.set(Some(true)));
        let thick = focus_ring_width();
        TEST_HIGH.with(|h| h.set(None));
        assert!(thick > normal);
    }

    #[test]
    fn a_test_with_its_own_palette_leaves_the_global_one_alone() {
        own_palette();
        set_high_contrast(true);
        assert!(high_contrast());
        // Another thread has no override of its own: it still sees the global setting.
        assert!(!std::thread::spawn(high_contrast).join().unwrap());
        set_high_contrast(false);
        assert!(!high_contrast());
    }

    #[test]
    fn hover_lifts_a_surface_one_step_and_pressing_two_never_past_the_brightest() {
        assert_eq!(lighter(SURFACE).0, SURFACE_HIGH.0);
        assert_eq!(lighter(SURFACE_HIGH).0, SURFACE_HIGHEST.0);
        assert_eq!(lighter(SURFACE_BRIGHT).0, SURFACE_BRIGHT.0, "the top of the ladder holds");
        assert_eq!(lighter(NIGHT).0, SURFACE.0, "the page background steps to a card surface");
        assert_eq!(state_fill(SURFACE, false, false).0, SURFACE.0);
        assert_eq!(state_fill(SURFACE, true, false).0, SURFACE_HIGH.0);
        assert_eq!(state_fill(SURFACE, true, true).0, SURFACE_HIGHEST.0);
        // Every step is a visible change and text stays readable on the brightest.
        for (look, floor) in LOOKS {
            for pair in LADDER.windows(2) {
                let (low, high) = (color(look, pair[0]), color(look, pair[1]));
                // OLED black's lower steps are all black; from there up each step shows.
                if look == Look::Oled && high == Color32::BLACK {
                    continue;
                }
                assert_ne!(low, high, "{look:?}: {:?} and {:?}", pair[0], pair[1]);
            }
            for text in [ON_SURFACE, ON_SURFACE_VARIANT] {
                let ratio = contrast(color(look, text), color(look, SURFACE_BRIGHT));
                assert!(ratio >= floor, "{look:?}: {text:?} on the brightest surface is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn hover_draws_no_outline_and_focus_stays_the_amber_ring() {
        let ctx = egui::Context::default();
        own_palette();
        for look in [Look::Night, Look::Day, Look::Oled] {
            set_theme_look(&ctx, look, false);
            let v = ctx.global_style().visuals.clone();
            assert_eq!(v.widgets.hovered.bg_stroke, Stroke::NONE, "{look:?}: no outline on hover");
            assert_eq!(v.widgets.active.bg_stroke.color, color(look, MOON), "{look:?}");
        }
        set_look(&ctx, true, false);
        let v = ctx.global_style().visuals.clone();
        assert_eq!(v.widgets.hovered.bg_stroke, v.widgets.inactive.bg_stroke, "high contrast keeps the outline every control has, no brighter one");
    }

    #[test]
    fn high_contrast_outlines_are_visible_against_the_surfaces() {
        for ground in [NIGHT, SURFACE, SURFACE_HIGH] {
            let ratio = contrast(color(Look::HighContrast, OUTLINE_VARIANT), color(Look::HighContrast, ground));
            assert!(ratio >= 3.0, "outline on {ground:?} is {ratio:.2}:1, wants 3");
        }
    }

    /// The bird's mask holds as much as its polygon, at the place the moon's box puts it.
    #[test]
    fn the_birds_mask_covers_what_its_outline_does() {
        let polygon = bird_polygon();
        let twice: f32 = polygon
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let b = polygon[(i + 1) % polygon.len()];
                a.0 * b.1 - b.0 * a.1
            })
            .sum();
        let area = (twice / 2.0).abs();
        let size = 256;
        let mask = bird_mask(size);
        let covered: f32 = mask.pixels.iter().map(|p| f32::from(p.a()) / 255.0).sum();
        let units = covered * (54.0 / size as f32).powi(2);
        assert!((units - area).abs() / area < 0.03, "{units} of {area}");
        assert!(mask.pixels.iter().any(|p| p.a() == 255) && mask.pixels.iter().any(|p| p.a() == 0));
    }

    #[test]
    fn logo_is_a_moon_with_a_night_bird() {
        let size = 64;
        let px = logo_rgba(size);
        let at = |x: usize, y: usize| &px[(y * size + x) * 4..(y * size + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corners are transparent");
        let moon = NORMAL[Index::MOON as usize];
        assert_eq!(at(32, 10), &[moon.r(), moon.g(), moon.b(), 255]);
        // The bird's body, just below the centre.
        let body = at(33, 40);
        assert!(body[0] < 80, "bird pixels are dark: {body:?}");
    }

    /// build.rs embeds this file as nightjar.exe's icon; the shell picks the size it needs from
    /// small taskbar and title bar icons up to Explorer's extra large view.
    #[test]
    fn windows_icon_has_every_shell_size() {
        let ico = include_bytes!("../packaging/icons/nightjar.ico");
        let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
        assert_eq!((u16_at(0), u16_at(2)), (0, 1), "an icon directory");
        let mut sizes = Vec::new();
        for entry in 0..usize::from(u16_at(4)) {
            let at = 6 + entry * 16;
            // A side of 0 means 256.
            let side = |byte: u8| if byte == 0 { 256 } else { usize::from(byte) };
            let (width, height) = (side(ico[at]), side(ico[at + 1]));
            assert_eq!(width, height, "square images");
            assert_eq!(u16_at(at + 6), 32, "{width} px has an alpha channel");
            assert!(u32_at(at + 12) + u32_at(at + 8) <= ico.len(), "{width} px lies inside the file");
            sizes.push(width);
        }
        for size in [16, 20, 24, 32, 40, 48, 64, 128, 256] {
            assert!(sizes.contains(&size), "{size} px missing from {sizes:?}");
        }
    }

    /// Whether the face at `index` in `bytes` maps `c` to a glyph.
    fn covers(bytes: &[u8], index: u32, c: char) -> bool {
        use skrifa::MetadataProvider;
        skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(c).is_some())
    }

    fn figtree_covers(c: char) -> bool {
        covers(FIGTREE, 0, c) && covers(FIGTREE_SEMIBOLD, 0, c)
    }

    /// Scripts written with spaces between words, where one word may not mix two faces.
    fn alphabetic_script(c: char) -> bool {
        matches!(u32::from(c), 0x0041..=0x024F | 0x0370..=0x052F | 0x1E00..=0x1EFF)
    }

    #[test]
    fn a_language_drawn_with_figtree_first_has_no_letter_figtree_lacks() {
        // Otherwise a word mixes Figtree with a lighter face. CJK may add its own scripts behind it.
        for lang in Lang::all() {
            let ahead = fallback(lang).is_some_and(|f| f.ahead);
            let lacking: std::collections::BTreeSet<char> =
                lang.texts().flat_map(str::chars).filter(|c| alphabetic_script(*c) && !figtree_covers(*c)).collect();
            if ahead {
                assert!(!lacking.is_empty(), "{lang:?} puts a system face ahead of Figtree without needing one");
            } else {
                assert!(lacking.is_empty(), "{lang:?} needs a system face ahead of Figtree for {lacking:?}");
            }
        }
    }

    #[test]
    fn every_text_of_every_language_has_its_glyphs() {
        // System fonts differ per machine: a language whose face is not installed here is skipped
        // with a note (Linux CI without Noto CJK), its selection is covered above.
        let mut checked = 0;
        for lang in Lang::all() {
            let system = system_fonts(lang);
            if fallback(lang).is_some() && system.regular.is_none() {
                eprintln!("skipped {lang:?}: its system font is not installed here");
                continue;
            }
            // Each letter of a language drawn ahead of Figtree comes from its system face.
            if let Some(face) = system.regular.as_ref().filter(|_| system.ahead) {
                let bytes = (*face.font).as_ref();
                let lacking: std::collections::BTreeSet<char> = lang
                    .texts()
                    .flat_map(str::chars)
                    .filter(|c| c.is_alphabetic() && !covers(bytes, face.index, *c))
                    .collect();
                assert!(lacking.is_empty(), "{lang:?}'s system face lacks {lacking:?}");
            }
            let ctx = egui::Context::default();
            ctx.set_fonts(definitions(system));
            ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
            for font in [regular(text::BODY), semibold(text::BODY)] {
                let missing: std::collections::BTreeSet<char> = lang
                    .texts()
                    .chain([lang.name()])
                    .flat_map(str::chars)
                    .filter(|c| !c.is_control() && !ctx.fonts_mut(|f| f.has_glyph(&font, *c)))
                    .collect();
                assert!(missing.is_empty(), "{lang:?} draws {missing:?} as boxes in {font:?}");
            }
            checked += 1;
        }
        // A machine without any of the fonts still checks the languages Figtree draws alone.
        assert!(checked >= Lang::all().filter(|l| fallback(*l).is_none()).count());
    }

    #[test]
    fn a_face_egui_cannot_read_is_dropped_instead_of_panicking() {
        // A WOFF2 or broken file, and a collection index past its faces.
        let mut bad_index = FontData::from_static(FIGTREE);
        bad_index.index = 7;
        let system = SystemFonts { ahead: true, regular: Some(FontData::from_owned(b"not a font".to_vec())), semibold: Some(bad_index) };
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions(system));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert!(ctx.fonts_mut(|f| f.has_glyph(&regular(text::BODY), 'A') && f.has_glyph(&semibold(text::BODY), 'A')));
    }

    #[test]
    fn fontconfig_s_named_instance_index_picks_the_face_and_its_axes() {
        assert_eq!(face_data(FIGTREE.to_vec(), 0).map(|d| d.index), Some(0));
        // Figtree is static: its "first named instance" index still names face 0, without coordinates.
        assert_eq!(face_data(FIGTREE.to_vec(), 1 << 16).map(|d| d.index), Some(0));
        assert!(face_data(FIGTREE.to_vec(), 3).is_none());
        assert!(face_data(b"wOF2 not really".to_vec(), 0).is_none());
    }

    #[test]
    fn a_language_s_faces_load_off_the_ui_thread() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let started = Instant::now();
        fonts_for(&ctx, Lang::Ja);
        // The first call only starts the loader, however long the files take.
        while ctx.data(|d| d.get_temp::<Lang>(fonts_key())) != Some(Lang::Ja) {
            assert!(started.elapsed().as_secs() < 30, "the Japanese fonts never loaded");
            std::thread::sleep(std::time::Duration::from_millis(10));
            fonts_for(&ctx, Lang::Ja);
        }
        // Another language lets the Japanese files go, and the loader is done.
        fonts_for(&ctx, Lang::En);
        assert!(!LOADED.lock().unwrap().iter().any(|e| e.0 == Lang::Ja));
        assert!(!LOADING.lock().unwrap().contains(&Lang::Ja));
    }

    #[test]
    fn a_loader_that_panics_lets_its_language_go() {
        LOADING.lock().unwrap().push(Lang::Vi);
        let ended = std::thread::spawn(|| {
            let _loading = Loading(Lang::Vi);
            panic!("a font file that breaks the loader");
        })
        .join();
        assert!(ended.is_err());
        assert!(!LOADING.lock().unwrap_or_else(PoisonError::into_inner).contains(&Lang::Vi));
    }

    #[test]
    fn the_fonts_follow_the_language_once_per_change() {
        let ctx = egui::Context::default();
        apply(&ctx);
        fonts_for(&ctx, Lang::De);
        assert_eq!(ctx.data(|d| d.get_temp::<Lang>(fonts_key())), Some(Lang::De));
        // A new palette or test rebuilds the fonts, and the next frame loads the language's again.
        apply(&ctx);
        assert_eq!(ctx.data(|d| d.get_temp::<Lang>(fonts_key())), None);
    }
}
