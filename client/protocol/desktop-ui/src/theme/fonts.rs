//! Figtree font definitions and asynchronous per-language system fallback font loading.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use egui::{FontData, FontDefinitions, FontFamily, FontId};

use crate::strings::Lang;
use super::{high_contrast, set_look, FIGTREE, FIGTREE_SEMIBOLD};

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}


/// The language whose system faces the context's fonts hold ([`fonts_for`]).
pub fn fonts_key() -> egui::Id {
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
pub static LOADED: Mutex<Vec<(Lang, SystemFonts)>> = Mutex::new(Vec::new());
/// The languages a thread loads faces for.
pub static LOADING: Mutex<Vec<Lang>> = Mutex::new(Vec::new());

/// Takes a language off `LOADING` when its loader ends, panicking or not. A loader that panicked
/// leaves the language without system faces, so the UI stops waiting instead of retrying.
pub struct Loading(pub Lang);

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
pub(super) fn loaded(ctx: &egui::Context, lang: Lang) -> Option<SystemFonts> {
    if !needs_fallback(lang) {
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
            let system = load_system_fonts(lang);
            LOADED.lock().unwrap_or_else(PoisonError::into_inner).push((lang, system));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            eprintln!("nightjar: could not start the font loader ({e}); loading the {} fonts here", lang.name());
            loading.retain(|l| *l != lang);
            let system = load_system_fonts(lang);
            LOADED.lock().unwrap_or_else(PoisonError::into_inner).push((lang, system.clone()));
            return Some(system);
        }
    }
    None
}

/// Figtree has Latin-1 only: no Cyrillic, no Polish, Turkish or Vietnamese letters, no CJK.
/// egui's own Ubuntu-Light draws some of them in a lighter weight, and none of CJK.
#[derive(Clone, Default)]
pub struct SystemFonts {
    /// Ahead of Figtree, so each word comes from one face: the language's words mix Figtree's
    /// letters with ones it lacks (Polish, Turkish, Vietnamese) or are Cyrillic, whose digits
    /// and punctuation would otherwise be Figtree's.
    pub ahead: bool,
    pub regular: Option<FontData>,
    pub semibold: Option<FontData>,
}

pub type FontLoaderFn = fn(Lang) -> SystemFonts;
pub type FontValidateFn = fn(&FontData) -> bool;

static FONT_LOADER: OnceLock<(FontLoaderFn, FontValidateFn)> = OnceLock::new();

/// Registers the OS/skrifa font loader and validator implemented in `nightjar-desktop::font_loader`.
pub fn register_font_loader(loader: FontLoaderFn, validate: FontValidateFn) {
    let _ = FONT_LOADER.set((loader, validate));
}

/// Whether `lang` needs a system fallback face (ahead of Figtree = true, behind = false, none = None).
pub fn fallback_ahead(lang: Lang) -> Option<bool> {
    match lang {
        Lang::En | Lang::De | Lang::Es | Lang::Fr | Lang::It | Lang::PtBr => None,
        Lang::Pl | Lang::Ru | Lang::Tr | Lang::Uk | Lang::Vi => Some(true),
        Lang::Ja | Lang::Ko | Lang::ZhHans | Lang::ZhHant => Some(false),
    }
}

pub fn needs_fallback(lang: Lang) -> bool {
    fallback_ahead(lang).is_some()
}

pub(super) fn load_system_fonts(lang: Lang) -> SystemFonts {
    match FONT_LOADER.get() {
        Some((loader, _)) => loader(lang),
        None => SystemFonts::default(),
    }
}

pub fn definitions(system: SystemFonts) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("figtree".into(), Arc::new(FontData::from_static(FIGTREE)));
    fonts.font_data.insert("figtree-semibold".into(), Arc::new(FontData::from_static(FIGTREE_SEMIBOLD)));
    let egui_fonts = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    // Drop unparseable font faces if a validator is registered (`font_loader` checks via `skrifa`).
    let usable = |data: Option<FontData>| {
        data.filter(|data| {
            let ok = FONT_LOADER.get().is_none_or(|(_, validate)| validate(data));
            if !ok {
                eprintln!("nightjar: a system font (face {}) cannot be read; drawing without it", data.index);
            }
            ok
        })
    };
    let (regular, semibold) = (usable(system.regular), usable(system.semibold));
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
        list.push("Hack".into());
        list
    };
    let proportional = family("figtree", regular);
    let bold = family("figtree-semibold", semibold);
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name("semibold".into()), bold);
    fonts
}
