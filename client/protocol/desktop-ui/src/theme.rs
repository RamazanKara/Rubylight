//! Nightjar's look on the desktop: the Android app's palettes (the night one's blue-black
//! surfaces and the amber of the brand moon, the day one's light glass, OLED black and high
//! contrast; Android's `AppTheme.Look`), Figtree type, and the brand mark drawn from its vector path.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use egui::{Color32, FontFamily, FontId, Stroke, TextStyle};

use crate::ui::design_tokens as t;
#[cfg(test)]
use crate::strings::Lang;

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
    /// (`system_light`), Light stays the day look whatever the system says, as Android's
    /// `AppTheme.look`; high contrast wins over every theme.
    pub fn of(theme: crate::config::Theme, system_light: bool, high: bool) -> Self {
        use crate::config::Theme;
        match theme {
            _ if high => Self::HighContrast,
            Theme::System if system_light => Self::Day,
            Theme::System | Theme::Dark => Self::Night,
            Theme::Oled => Self::Oled,
            Theme::Light => Self::Day,
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

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// A test's own palette, so a test looking at high contrast does not recolour the others.
    static TEST_HIGH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// The same for the theme's look.
    static TEST_BASE: std::cell::Cell<Option<Look>> = const { std::cell::Cell::new(None) };
    /// And for drawing over video.
    static TEST_OVER: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn high_contrast() -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(high) = TEST_HIGH.with(std::cell::Cell::get) {
        return high;
    }
    HIGH_CONTRAST.load(Ordering::Relaxed)
}

pub fn set_high_contrast(on: bool) {
    // A test with its own palette changes only that, never what the other tests see.
    #[cfg(any(test, feature = "test-support"))]
    if TEST_HIGH.with(|h| h.get().is_some()) {
        TEST_HIGH.with(|h| h.set(Some(on)));
        return;
    }
    HIGH_CONTRAST.store(on, Ordering::Relaxed);
}

/// The theme's look, high contrast aside.
fn base_look() -> Look {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(look) = TEST_BASE.with(std::cell::Cell::get) {
        return look;
    }
    Look::from_u8(BASE_LOOK.load(Ordering::Relaxed))
}

fn set_base_look(look: Look) {
    let look = if look == Look::HighContrast { Look::Night } else { look };
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    if let Some(over) = TEST_OVER.with(std::cell::Cell::get) {
        return over;
    }
    OVER_VIDEO.load(Ordering::Relaxed)
}

/// Sets whether frames draw over video; answers whether they did.
fn swap_over_video(over_video: bool) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if TEST_OVER.with(|o| o.get().is_some()) {
        return TEST_OVER.with(|o| o.replace(Some(over_video))).unwrap_or(false);
    }
    OVER_VIDEO.swap(over_video, Ordering::Relaxed)
}

/// Test only: gives this thread its own palette setting, night, not high contrast and not over
/// video at first.
#[cfg(any(test, feature = "test-support"))]
pub fn own_palette() {
    TEST_HIGH.with(|h| h.set(Some(false)));
    TEST_BASE.with(|b| b.set(Some(Look::Night)));
    TEST_OVER.with(|o| o.set(Some(false)));
}

/// Test only: this thread's palette (see [`own_palette`]) in `look`.
#[cfg(any(test, feature = "test-support"))]
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

static REDUCES_MOTION_HOOK: OnceLock<fn() -> bool> = OnceLock::new();

/// Registers the OS reduced-motion preference reader from `nightjar-desktop::font_loader`.
pub fn register_reduces_motion_hook(hook: fn() -> bool) {
    let _ = REDUCES_MOTION_HOOK.set(hook);
}

fn os_reduces_motion() -> bool {
    REDUCES_MOTION_HOOK.get().is_some_and(|f| f())
}


pub const FIGTREE: &[u8] = include_bytes!("../../desktop/assets/Figtree-400.ttf");
pub const FIGTREE_SEMIBOLD: &[u8] = include_bytes!("../../desktop/assets/Figtree-600.ttf");

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

mod controls;
mod fonts;

pub use controls::{
    bird_mask, log_slider, logo_image, logo_rgba, mbps_text, percent, slider, step_value,
    VALUE_AFTER_KEY,
};
pub use fonts::{
    apply, definitions, fallback_ahead, fonts_for, fonts_key, needs_fallback,
    register_font_loader, regular, semibold, FontLoaderFn, FontValidateFn, Loading,
    SystemFonts, LOADED, LOADING,
};

#[cfg(test)]
use std::sync::PoisonError;
#[cfg(test)]
use controls::{bird_polygon, inside, rail_position, value_floats, value_rect};

#[cfg(test)]
mod tests;
