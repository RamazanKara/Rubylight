//! The couch menu's extension rows (`docs/couch-menu-design.md` section 4.3).
//!
//! [`ITEMS`] is the one registration point. A host feature that can change at
//! runtime adds its row there with an [`Item`] whose `value` and `set` call
//! that feature's own getter and setter, so the menu holds no feature logic.
//! A feature that is not registered has no row: night mode, headphone
//! surround, encoder focus and sleep after idle have no runtime setter on
//! main yet, so the menu does not show them. Their labels are already in the
//! catalogue (`couch_menu/items/<key>`).
use crate::native_api::Failure;
use crate::state::{Host, Session};
use butterpollo_core::couch_menu::model::{Applies, Availability, Control};
use butterpollo_core::native_api::radeon::Value;
use butterpollo_core::state::Client;

/// One extension row.
pub(crate) trait Item: Sync {
    /// One of `couch_menu::model::items`; the label is the catalogue string
    /// at `items::label_path(key)`.
    fn key(&self) -> &'static str;
    /// `Hidden` when the feature does not apply to this session.
    fn available(&self, h: &Host, s: &Session) -> Availability;
    fn value(&self, h: &Host, s: &Session) -> Control;
    /// Runs on the menu worker. The caller's Host control is checked first
    /// when [`Item::needs_host_control`] says so.
    fn set(&self, h: &Host, s: &Session, client: &Client, value: Value) -> Result<(), Failure>;
    fn needs_host_control(&self) -> bool;
    fn applies(&self) -> Applies;
}

/// The registered rows, in menu order. Each feature adds one line.
pub(crate) static ITEMS: &[&dyn Item] = &[];

/// The registered row with `key`.
pub(crate) fn find<'a>(items: &[&'a dyn Item], key: &str) -> Option<&'a dyn Item> {
    items.iter().copied().find(|item| item.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::couch_menu::model::items;

    #[test]
    fn couch_menu_registered_rows_are_model_items_once_each() {
        for (at, item) in ITEMS.iter().enumerate() {
            assert!(
                items::ALL.contains(&item.key()),
                "{} is no model item",
                item.key()
            );
            assert!(
                ITEMS[..at].iter().all(|other| other.key() != item.key()),
                "{} is registered twice",
                item.key()
            );
        }
    }
}
