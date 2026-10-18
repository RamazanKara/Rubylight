//! Small LRU of immutable cursor shapes. Reusing a shape never overwrites a
//! texture still referenced by an older capture or queued GPU conversion.
use std::{collections::VecDeque, sync::Arc};

const MAX_SHAPES: usize = 16;
const MAX_BYTES: usize = 4 * 1024 * 1024;

struct Entry<T> {
    width: u32,
    height: u32,
    logic: bool,
    pixels: Arc<[u8]>,
    value: T,
}
pub(crate) struct ShapeCache<T> {
    entries: VecDeque<Entry<T>>,
    bytes: usize,
}
impl<T> Default for ShapeCache<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
        }
    }
}
impl<T> ShapeCache<T> {
    fn find(&self, width: u32, height: u32, logic: bool, pixels: &[u8]) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry.width == width
                && entry.height == height
                && entry.logic == logic
                && (std::ptr::eq(entry.pixels.as_ref(), pixels) || entry.pixels.as_ref() == pixels)
        })
    }
    pub fn get(&mut self, width: u32, height: u32, logic: bool, pixels: &[u8]) -> Option<T>
    where
        T: Clone,
    {
        let at = self.find(width, height, logic, pixels)?;
        let entry = self.entries.remove(at).unwrap();
        let value = entry.value.clone();
        self.entries.push_front(entry);
        Some(value)
    }
    pub fn insert(&mut self, width: u32, height: u32, logic: bool, pixels: Arc<[u8]>, value: T) {
        if pixels.len() > MAX_BYTES {
            return;
        }
        if let Some(at) = self.find(width, height, logic, &pixels) {
            self.bytes -= self.entries.remove(at).unwrap().pixels.len();
        }
        while self.entries.len() >= MAX_SHAPES || self.bytes + pixels.len() > MAX_BYTES {
            self.bytes -= self.entries.pop_back().unwrap().pixels.len();
        }
        self.bytes += pixels.len();
        self.entries.push_front(Entry {
            width,
            height,
            logic,
            pixels,
            value,
        });
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|entry| &entry.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_contents_reuse_shape_but_geometry_and_logic_remain_distinct() {
        let mut cache = ShapeCache::default();
        cache.insert(2, 2, false, vec![1; 16].into(), 7);
        // A newly decoded animation frame has a new allocation, but identical pixels.
        assert_eq!(cache.get(2, 2, false, &[1; 16]), Some(7));
        assert_eq!(cache.get(1, 4, false, &[1; 16]), None);
        assert_eq!(cache.get(2, 2, true, &[1; 16]), None);
        assert_eq!(cache.get(2, 2, false, &[2; 16]), None);
        cache.insert(2, 2, false, vec![1; 16].into(), 9);
        assert_eq!(cache.get(2, 2, false, &[1; 16]), Some(9));
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, 16);
    }

    #[test]
    fn count_eviction_preserves_recent_and_in_flight_shapes() {
        let mut cache = ShapeCache::default();
        for i in 0..MAX_SHAPES {
            cache.insert(1, 1, false, vec![i as u8; 4].into(), Arc::new(i));
        }
        let held = cache.get(1, 1, false, &[1; 4]).unwrap();
        // Touch every other entry so the held shape is the eviction victim.
        for i in (0..MAX_SHAPES).filter(|i| *i != 1) {
            assert!(cache.get(1, 1, false, &[i as u8; 4]).is_some());
        }
        cache.insert(1, 1, false, vec![16; 4].into(), Arc::new(16));
        assert!(cache.get(1, 1, false, &[1; 4]).is_none());
        assert_eq!(*held, 1);
        assert_eq!(Arc::strong_count(&held), 1);
        assert!(cache.get(1, 1, false, &[0; 4]).is_some());
        assert_eq!(cache.entries.len(), MAX_SHAPES);
    }

    #[test]
    fn byte_limit_bounds_large_shapes_and_clear_forgets_device_resources() {
        let mut cache = ShapeCache::default();
        cache.insert(768, 1024, false, vec![1; 3 * 1024 * 1024].into(), 1);
        cache.insert(512, 1024, false, vec![2; 2 * 1024 * 1024].into(), 2);
        assert_eq!(cache.values().copied().collect::<Vec<_>>(), [2]);
        assert_eq!(cache.bytes, 2 * 1024 * 1024);
        cache.insert(2048, 1024, false, vec![0; MAX_BYTES + 1].into(), 3);
        assert_eq!(cache.entries.len(), 1);
        cache.clear();
        assert_eq!(cache.bytes, 0);
        assert!(cache.values().next().is_none());
    }
}
