//! The launcher's words added for Nightjar 7.0. The tables in `strings/<code>.rs` may not grow, so
//! every new text gets its entry here instead, in the same format; [`crate::strings`] looks a key
//! up in both tables of a language, and a key lives in exactly one of them.
//! `python tools/i18n.py apply` writes new keys here.

pub(crate) mod de;
pub(crate) mod es;
pub(crate) mod fr;
pub(crate) mod it;
pub(crate) mod ja;
pub(crate) mod ko;
pub(crate) mod pl;
pub(crate) mod pt_br;
pub(crate) mod ru;
pub(crate) mod tr;
pub(crate) mod uk;
pub(crate) mod vi;
pub(crate) mod zh_hans;
pub(crate) mod zh_hant;
