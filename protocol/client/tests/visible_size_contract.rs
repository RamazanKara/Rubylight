//! The stream part of a padded decoded picture, from
//! `app/src/test/resources/protocol/visible-size.txt`, checked against `video::crop::visible_size`.
//! Android's `StreamResizePolicyTest` checks the same file against
//! `StreamResizePolicy.paddedVisibleSize`, so its upscaler and stream view crop as desktop does.

use nightjar_core::video::crop::visible_size;

const FIXTURE: &str = include_str!("../../../app/src/test/resources/protocol/visible-size.txt");

fn size(text: &str) -> (i32, i32) {
    let (width, height) = text.split_once('x').unwrap_or_else(|| panic!("{text:?}"));
    (width.parse().unwrap(), height.parse().unwrap())
}

#[test]
fn visible_size_crops_every_case_android_crops() {
    let mut cases = 0;
    for line in FIXTURE
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let fields: Vec<&str> = line.split(' ').collect();
        assert_eq!(fields.len(), 3, "{line:?}");
        let requested: Vec<(i32, i32)> = if fields[1] == "-" {
            Vec::new()
        } else {
            fields[1].split(',').map(size).collect()
        };
        assert_eq!(
            visible_size(size(fields[0]), &requested),
            size(fields[2]),
            "{line:?}"
        );
        cases += 1;
    }
    assert!(cases >= 20, "only {cases} cases");
}
