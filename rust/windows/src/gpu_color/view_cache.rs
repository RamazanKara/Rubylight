//! Shader resource views of capture textures, kept across conversions.
//! Captures come from small fixed pools and rings, so a view per texture is
//! made once instead of every frame. A cached view holds a COM reference to
//! its texture, so the texture's address, the key, cannot name another
//! texture while the entry lives.
use anyhow::Result;

/// More than any capture pool or ring hands out.
const CAPACITY: usize = 8;
/// Conversions an entry may go unused before its texture is let go, so a
/// pool replaced at the same size does not stay in VRAM.
const IDLE: u64 = 120;

struct Entry<V> {
    key: usize,
    view: V,
    used: u64,
}

pub(super) struct ViewCache<V, S> {
    entries: Vec<Entry<V>>,
    /// The source size and format the entries were made for: a change
    /// (a resize or another capture backend) drops them all.
    shape: Option<S>,
    uses: u64,
}

impl<V, S> Default for ViewCache<V, S> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            shape: None,
            uses: 0,
        }
    }
}

impl<V: Clone, S: Copy + PartialEq> ViewCache<V, S> {
    /// The view for the texture at `key`, made with `create` when not cached.
    pub(super) fn view(
        &mut self,
        key: usize,
        shape: S,
        create: impl FnOnce() -> Result<V>,
    ) -> Result<V> {
        if self.shape != Some(shape) {
            self.entries.clear();
            self.shape = Some(shape);
        }
        self.uses += 1;
        let now = self.uses;
        self.entries.retain(|entry| now - entry.used < IDLE);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.used = now;
            return Ok(entry.view.clone());
        }
        let view = create()?;
        if self.entries.len() >= CAPACITY
            && let Some(oldest) = (0..self.entries.len()).min_by_key(|&i| self.entries[i].used)
        {
            self.entries.swap_remove(oldest);
        }
        self.entries.push(Entry {
            key,
            view: view.clone(),
            used: now,
        });
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(cache: &mut ViewCache<u32, u8>, key: usize, shape: u8, next: &mut u32) -> bool {
        let before = *next;
        let view = cache
            .view(key, shape, || {
                *next += 1;
                Ok(*next)
            })
            .unwrap();
        assert!(view <= *next);
        *next != before
    }

    fn keys(cache: &ViewCache<u32, u8>) -> Vec<usize> {
        let mut keys: Vec<_> = cache.entries.iter().map(|entry| entry.key).collect();
        keys.sort();
        keys
    }

    #[test]
    fn a_ring_reuses_its_views() {
        let mut cache = ViewCache::default();
        let mut next = 0;
        for key in 1..=4 {
            assert!(made(&mut cache, key, 0, &mut next));
        }
        for _ in 0..50 {
            for key in 1..=4 {
                assert!(!made(&mut cache, key, 0, &mut next));
            }
        }
        assert_eq!(next, 4);
        let view = cache.view(2, 0, || unreachable!()).unwrap();
        assert_eq!(view, 2);
    }

    #[test]
    fn a_full_cache_drops_the_least_recently_used() {
        let mut cache = ViewCache::default();
        let mut next = 0;
        for key in 1..=CAPACITY {
            made(&mut cache, key, 0, &mut next);
        }
        made(&mut cache, 1, 0, &mut next);
        assert!(made(&mut cache, 100, 0, &mut next));
        assert_eq!(cache.entries.len(), CAPACITY);
        assert!(!keys(&cache).contains(&2));
        assert!(keys(&cache).contains(&1));
    }

    #[test]
    fn an_unused_view_is_dropped_after_the_idle_window() {
        let mut cache = ViewCache::default();
        let mut next = 0;
        made(&mut cache, 1, 0, &mut next);
        for _ in 0..IDLE - 1 {
            made(&mut cache, 2, 0, &mut next);
        }
        assert_eq!(keys(&cache), [1, 2], "used within the window");
        made(&mut cache, 2, 0, &mut next);
        assert_eq!(keys(&cache), [2]);
        assert!(made(&mut cache, 1, 0, &mut next));
    }

    #[test]
    fn a_new_source_shape_drops_every_view() {
        let mut cache = ViewCache::default();
        let mut next = 0;
        made(&mut cache, 1, 0, &mut next);
        made(&mut cache, 2, 0, &mut next);
        made(&mut cache, 3, 1, &mut next);
        assert_eq!(keys(&cache), [3]);
        assert!(made(&mut cache, 1, 1, &mut next));
    }

    #[test]
    fn a_failed_view_is_not_cached() {
        let mut cache: ViewCache<u32, u8> = ViewCache::default();
        assert!(cache.view(1, 0, || anyhow::bail!("no view")).is_err());
        assert!(cache.entries.is_empty());
        assert_eq!(cache.view(1, 0, || Ok(7)).unwrap(), 7);
    }
}
