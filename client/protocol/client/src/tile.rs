//! The no-art tile's choices, the same on every client: a game without a cover always gets the same
//! gradient and disc, and Android and the desktop agree on them.

/// FNV-1a over the title's lowercase UTF-8.
pub fn title_hash(title: &str) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    for byte in title.to_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Which of the twelve `tile.pair<N>` gradients the hash picks.
pub fn tile_pair(hash: u32) -> usize {
    (hash % 12) as usize
}

/// Where the tile's disc sits, as fractions of its width and height.
pub fn tile_disc_center(hash: u32) -> [f32; 2] {
    const CENTERS: [[f32; 2]; 4] = [[0.74, 0.62], [0.26, 0.7], [0.8, 0.42], [0.5, 0.78]];
    CENTERS[((hash >> 8) % 4) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_fnv_1a_and_ignores_case() {
        assert_eq!(title_hash(""), 0x811C_9DC5);
        assert_eq!(title_hash("a"), 0xE40C_292C);
        assert_eq!(title_hash("Frost Peak"), title_hash("frost peak"));
    }

    #[test]
    fn pair_and_disc_stay_in_range() {
        for title in ["Frost Peak", "Wayfarer", "Tavern Duel", "日本語"] {
            let hash = title_hash(title);
            assert!(tile_pair(hash) < 12);
            let [x, y] = tile_disc_center(hash);
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
        }
    }
}
