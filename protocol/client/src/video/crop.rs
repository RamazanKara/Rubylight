//! The part of a decoded picture that is the stream: an encoder that codes a larger picture than
//! it was asked for leaves alignment padding at the right and bottom edges, which a decoder hands
//! out as part of the picture when the bitstream does not say otherwise.
//!
//! AMD's AV1 encoder rounds the width up to 64 columns and the height up to 16 rows (two rows for
//! its eight-row special case: 1920x1080 is coded as 1920x1082, 2184x1968 as 2240x1968), with no
//! render size in the frame header (`rust/PERFORMANCE_WORK.md`, "AV1 padding at 1968x2184").
//! H.264 and HEVC carry a cropping window that FFmpeg applies, so their pictures are the stream's
//! size already; a decoder that skipped it would leave at most 15 rows of macroblock padding.

/// The most columns an encoder's alignment adds (AMD AV1: up to a multiple of 64).
pub const PADDING_COLUMNS: i32 = 64;
/// The most rows an encoder's alignment adds (AMD AV1 and H.264 macroblocks: a multiple of 16).
pub const PADDING_ROWS: i32 = 16;

/// The size of `decoded` to show: the first of the `requested` stream sizes it is that size plus
/// alignment padding of, or the whole picture. A picture that is one of the requested sizes, smaller
/// than all of them or larger by more than padding (a stream at another size) is shown whole.
pub fn visible_size(decoded: (i32, i32), requested: &[(i32, i32)]) -> (i32, i32) {
    if requested.contains(&decoded) {
        return decoded;
    }
    let padded = |extent: i32, asked: i32, most: i32| asked > 0 && (asked..asked + most).contains(&extent);
    requested
        .iter()
        .copied()
        .find(|&(width, height)| padded(decoded.0, width, PADDING_COLUMNS) && padded(decoded.1, height, PADDING_ROWS))
        .unwrap_or(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amds_padded_av1_pictures_show_the_stream_size() {
        // The RX 7900 XT's AV1 sizes measured in rust/PERFORMANCE_WORK.md.
        assert_eq!(visible_size((2240, 1968), &[(2184, 1968)]), (2184, 1968));
        assert_eq!(visible_size((1984, 2186), &[(1968, 2184)]), (1968, 2184));
        assert_eq!(visible_size((1920, 1082), &[(1920, 1080)]), (1920, 1080));
    }

    #[test]
    fn a_picture_of_the_stream_size_is_shown_whole() {
        assert_eq!(visible_size((1920, 1080), &[(1920, 1080)]), (1920, 1080));
        assert_eq!(visible_size((2184, 1968), &[(2184, 1968)]), (2184, 1968));
    }

    #[test]
    fn a_smaller_picture_is_never_cropped_up() {
        assert_eq!(visible_size((1280, 720), &[(1920, 1080)]), (1280, 720));
        assert_eq!(visible_size((2176, 1968), &[(2184, 1968)]), (2176, 1968));
        assert_eq!(visible_size((2240, 1960), &[(2184, 1968)]), (2240, 1960));
    }

    #[test]
    fn a_picture_of_another_size_is_not_padding() {
        // More than one alignment step larger: a stream at another size (a mode change on its way).
        assert_eq!(visible_size((2560, 1440), &[(1920, 1080)]), (2560, 1440));
        assert_eq!(visible_size((1984, 1080), &[(1920, 1080)]), (1984, 1080));
        assert_eq!(visible_size((1920, 1096), &[(1920, 1080)]), (1920, 1096));
        assert_eq!(visible_size((1920, 1082), &[]), (1920, 1082));
        assert_eq!(visible_size((64, 16), &[(0, 0)]), (64, 16));
    }

    #[test]
    fn a_mode_change_crops_to_the_size_the_picture_is_of() {
        // Pictures of the old size keep arriving after the new one was asked for, and the other way.
        let sizes = [(2184, 1968), (1920, 1080)];
        assert_eq!(visible_size((1920, 1082), &sizes), (1920, 1080));
        assert_eq!(visible_size((2240, 1968), &sizes), (2184, 1968));
        assert_eq!(visible_size((1920, 1080), &[(1900, 1070), (1920, 1080)]), (1920, 1080));
    }
}
