//! Cross-host Console library stores (`LibraryCacheStore`, `RecentsStore`, `FavouritesStore`,
//! `HiddenStore`) and `ConsoleRepository` with `std::thread::scope` host refreshes and
//! 5-minute capability cooldown tracking.

pub mod repo;
pub mod store;
mod transport;

#[cfg(test)]
mod tests;

pub use nightjar_policy::console::{
    build_hidden_listings, classic_app, classic_library, continue_playing, detail, entry_key_of,
    favourite_for, favourite_games, favourite_of, hide_keys_by_id, is_hidden_entry, neighbour,
    not_hidden_by_id, recheck_running, reconcile_starred, saved_classic_app, split_hidden,
    ANDROID_RECENT_LIMIT, CLASSIC_REVISION, ClassicApp, ClassicApps, Continue, Copy, Detail,
    EndAction, Entry, Favourite, GameState, HiddenEntry, HiddenListing, HiddenTracker, HomeFilterKey,
    HomeGroup, HomeOrder, HomeProjection, HomeSort, LibraryFilter, Playing, ProjectedCard,
    ProjectedChip, Recent, Recently, RichSource, Running, Sort, Source, Starred,
};
pub use repo::{
    ConsoleRepository, ConsoleSnapshot, HostClientTransport, HostDescriptor, NativeTransport,
    FEATURES_RETRY_COOLDOWN,
};
pub use store::{
    find_cached_library_cover, forget_host, has_any_library, has_classic_app_list, has_library,
    sanitize_host_id, CachedLibraryCover, FavouritesStore, HiddenStore, LibraryCacheStore,
    RecentsStore,
};
