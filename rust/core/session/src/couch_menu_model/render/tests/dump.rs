//! The review dump: with `RUBYLIGHT_COUCH_MENU_DUMP_DIR` set, writes each
//! menu state as the viewer sees it (the 1x panel scaled by `frame.scale`,
//! nearest neighbour, at its origin over a game-like mid-tone picture of the
//! full target size) as PNGs at 1280x720, 1920x1080 and 3840x2160. Without
//! the variable it does nothing.
use std::path::Path;

use super::super::super::model::tests::{HOST_STRINGS, snapshot};
use super::super::super::model::{Snapshot, Strings};
use super::*;

/// The couch menu's own strings in `language` from the host catalogue, as
/// `locale::catalogue` picks them: German for `de`, English for any other.
fn catalogue(language: &str) -> Strings {
    let file = if language == "de" { "de" } else { "en" };
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../assets/locale/{file}.json"));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    Strings::from_lookup(|key| {
        key.split('/')
            .try_fold(&json, |node, part| node.get(part))?
            .as_str()
            .map(str::to_owned)
    })
    .unwrap()
}

/// The tests' snapshot with the host's labels in German.
fn german(mut snap: Snapshot) -> Snapshot {
    let de = |text: &mut String| {
        if let Some((_, de)) = HOST_STRINGS.iter().find(|(en, _)| en == text) {
            *text = (*de).to_owned();
        }
    };
    let settings = snap.preset.iter_mut().chain(&mut snap.items);
    for setting in settings {
        de(&mut setting.label);
        if let super::super::super::Control::Choice { options, .. } = &mut setting.control {
            options.iter_mut().for_each(|o| de(&mut o.label));
        }
    }
    snap
}

/// The tests' snapshot with both audio items, one on.
fn switches() -> Snapshot {
    let mut snap = snapshot();
    let mut surround = snap.items[0].clone();
    surround.id = super::super::super::items::AUDIO_HEADPHONE_SURROUND.into();
    surround.label = "Headphone surround".into();
    surround.control = super::super::super::Control::Toggle(true);
    snap.items.push(surround);
    snap
}

/// Each state for review, by name.
fn states(layout: Layout) -> Vec<(&'static str, Frame)> {
    let open =
        |language: &str, snap: Snapshot| Driver::new(Menu::new(catalogue(language)), snap, layout);
    let mut out = Vec::new();
    let run = open("en", snapshot());
    out.push(("main", run.frame()));

    let mut run = open("en", snapshot());
    run.focus("Radeon settings");
    run.nav(Nav::Accept);
    run.focus("Image sharpening");
    out.push(("radeon-slider", run.frame()));
    run.focus("Radeon Chill");
    out.push(("locked-focused", run.frame()));

    let mut run = open("en", switches());
    run.focus("Night mode");
    out.push(("switches-off-focused", run.frame()));
    run.focus("Headphone surround");
    out.push(("switches-on-focused", run.frame()));
    run.focus("Stream preset");
    out.push(("choice-focused", run.frame()));

    let mut run = open("en", snapshot());
    run.focus("End session");
    run.nav(Nav::Accept);
    out.push(("confirm", run.frame()));

    let mut run = open("en", snapshot());
    run.focus("Night mode");
    assert!(run.nav(Nav::Accept).is_some());
    out.push(("busy", run.frame()));
    run.menu.finish(Err("audio device not found".into()));
    out.push(("error", run.frame()));

    let mut run = open("de", german(switches()));
    out.push(("de-main", run.frame()));
    run.focus("Radeon");
    run.nav(Nav::Accept);
    run.focus("Image sharpening");
    out.push(("de-radeon", run.frame()));
    // The menu has English and German only: a Russian host draws English.
    out.push(("ru-main", open("ru", snapshot()).frame()));
    out
}

/// `frame` over a mid-tone picture of the target size, as RGB8.
fn composite(frame: &Frame) -> Vec<u8> {
    let [width, height] = frame.target;
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let panel_width = frame.size[0] / frame.scale;
    let inside = |v: u32, i: usize| (frame.origin[i]..frame.origin[i] + frame.size[i]).contains(&v);
    for y in 0..height {
        for x in 0..width {
            if inside(x, 0) && inside(y, 1) {
                let px = (x - frame.origin[0]) / frame.scale;
                let py = (y - frame.origin[1]) / frame.scale;
                let at = ((py * panel_width + px) * 4) as usize;
                rgb.extend_from_slice(&frame.rgba[at..at + 3]);
            } else {
                // A dusky landscape: sky to ground, soft bands.
                let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
                let band = ((fx * 9. + fy * 5.).sin() * 10.) as i32;
                let sky = [92., 118., 140.].map(|c: f32| c - 30. * fy);
                let ground = [128., 108., 78.].map(|c: f32| c + 20. * fx);
                let mix = ((fy - 0.55) * 6.).clamp(0., 1.);
                for c in 0..3 {
                    let v = sky[c] + (ground[c] - sky[c]) * mix;
                    rgb.push((v as i32 + band).clamp(0, 255) as u8);
                }
            }
        }
    }
    rgb
}

fn write_png(path: &Path, [width, height]: [u32; 2], rgb: &[u8]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgb).unwrap();
}

#[test]
fn couch_menu_render_dump() {
    let Some(dir) = std::env::var_os("RUBYLIGHT_COUCH_MENU_DUMP_DIR") else {
        return;
    };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    for [w, h] in [[1280, 720], [1920, 1080], [3840, 2160]] {
        let layout = Layout::for_stream(w, h).unwrap();
        for (name, frame) in states(layout) {
            let path = dir.join(format!("{w}x{h}-{name}.png"));
            write_png(&path, frame.target, &composite(&frame));
            println!("{}", path.display());
        }
    }
}
