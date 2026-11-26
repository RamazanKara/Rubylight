//! `frame_warp.json`: saving, loading, the schema 1 migration and damaged files, in a temporary
//! directory removed after each test.

use butterpollo_media_policy::frame_warp::Prior;

use super::*;

/// A directory of its own under the temporary directory, removed however the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("frame-warp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn store(&self, key: Option<&str>) -> Store {
        Store::new(&self.0, key.map(str::to_owned))
    }

    fn file(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.0.join(FILE)).unwrap()).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prior(gain: f32) -> PriorV2 {
    PriorV2 {
        base: Prior {
            mouse_gain: Some([gain, gain]),
            mouse_lag_us: Some(32_000),
            picture_width: 1920,
            picture_height: 1080,
            ..Prior::default()
        },
        ..PriorV2::default()
    }
}

/// What a stream saves and the next loads: the stamp aside, the same.
fn unstamped(prior: Option<PriorV2>) -> Option<PriorV2> {
    prior.map(|prior| PriorV2 {
        confident_at: None,
        ..prior
    })
}

#[test]
fn the_least_recently_used_game_goes_first() {
    let mut remembered = Remembered::default();
    for n in 0..GAMES as u64 {
        remembered.remember(&format!("uuid:{n}"), prior(1.0), 1_000 + n);
    }
    // Game 0 is used again, so game 1 is the oldest when one more comes.
    remembered.remember("uuid:0", prior(2.0), 5_000);
    remembered.remember("name:Another", prior(3.0), 5_001);
    assert_eq!(remembered.games.len(), GAMES);
    assert!(!remembered.games.contains_key("uuid:1"));
    assert_eq!(remembered.games["uuid:0"].prior, prior(2.0));
    assert_eq!(remembered.games["name:Another"].used, 5_001);
}

#[test]
fn a_saved_prior_with_its_gyro_loads_for_the_same_game_only() {
    let scratch = Scratch::new();
    let gyro = PriorV2 {
        gyro_gain: Some([-825.0, -790.0]),
        gyro_lag_us: Some(44_000),
        gyro_yaw_z: Some(true),
        ..prior(1.5)
    };
    assert_eq!(scratch.store(Some("uuid:a")).load(), None);
    scratch.store(Some("uuid:a")).save(gyro, false);
    scratch.store(None).save(prior(9.0), true);
    assert_eq!(scratch.store(Some("uuid:a")).load(), Some(gyro));
    assert_eq!(scratch.store(Some("uuid:b")).load(), None);
    assert_eq!(scratch.store(None).load(), None);
    let file = scratch.file();
    assert_eq!(file["schema"], 2);
    assert_eq!(file["games"].as_object().unwrap().len(), 1);
    // Saved while locked, the entry is stamped; a later save while unlocked keeps the stamp.
    scratch.store(Some("uuid:a")).save(gyro, true);
    let stamped = scratch.store(Some("uuid:a")).load().unwrap();
    assert!(stamped.confident_at.is_some_and(|at| at > 1_600_000_000));
    scratch.store(Some("uuid:a")).save(prior(2.0), false);
    let kept = scratch.store(Some("uuid:a")).load().unwrap();
    assert_eq!(kept.confident_at, stamped.confident_at);
    assert_eq!(unstamped(Some(kept)), Some(prior(2.0)));
    // No temporary file is left behind.
    let names: Vec<_> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [FILE]);
}

#[test]
fn a_schema_1_file_reads_without_the_gyro_and_is_saved_as_schema_2() {
    let scratch = Scratch::new();
    std::fs::write(
        scratch.0.join(FILE),
        br#"{"games":{"uuid:a":{"prior":{"mouse_gain":[1.5,1.5],"mouse_lag_us":32000,
            "stick_gain":null,"stick_lag_us":null,"stick_deadzone":null,"stick_exponent":null,
            "focal":960.0,"picture_width":1920,"picture_height":1080},"used":1000},
            "name:Old":{"prior":{"mouse_gain":null,"mouse_lag_us":null,"stick_gain":[300.0,300.0],
            "stick_lag_us":40000,"stick_deadzone":0.1,"stick_exponent":1.5,
            "picture_width":1280,"picture_height":720},"used":900}}}"#,
    )
    .unwrap();
    let a = scratch.store(Some("uuid:a")).load().unwrap();
    assert_eq!(
        a,
        PriorV2 {
            base: Prior {
                focal: Some(960.0),
                ..prior(1.5).base
            },
            ..PriorV2::default()
        }
    );
    let old = scratch.store(Some("name:Old")).load().unwrap();
    assert_eq!(old.base.stick_exponent, Some(1.5));
    assert_eq!(old.gyro_gain, None);
    scratch.store(Some("uuid:b")).save(prior(2.0), false);
    let file = scratch.file();
    assert_eq!(file["schema"], 2);
    assert_eq!(file["games"].as_object().unwrap().len(), 3);
    assert_eq!(scratch.store(Some("uuid:a")).load(), Some(a));
    assert_eq!(scratch.store(Some("name:Old")).load(), Some(old));
}

#[test]
fn a_file_that_does_not_parse_is_kept_aside_and_the_game_starts_cold() {
    let scratch = Scratch::new();
    let path = scratch.0.join(FILE);
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(scratch.store(Some("uuid:a")).load(), None);
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(scratch.0.join(CORRUPT)).unwrap(),
        b"{not json"
    );
    // The next save starts a file of its own.
    scratch.store(Some("uuid:b")).save(prior(2.5), false);
    assert_eq!(
        unstamped(scratch.store(Some("uuid:b")).load()),
        Some(prior(2.5))
    );
    assert_eq!(scratch.file()["schema"], 2);
}

#[test]
fn save_skips_instead_of_overwriting_when_existing_file_cannot_be_read() {
    let scratch = Scratch::new();
    let path = scratch.0.join(FILE);
    std::fs::create_dir_all(&path).unwrap();
    let store = scratch.store(Some("uuid:a"));
    assert_eq!(store.load(), None);
    store.save(prior(1.5), true);
    assert!(
        path.is_dir(),
        "unreadable directory must not be overwritten"
    );
}

#[test]
fn a_newer_hosts_file_is_read_but_not_written_over() {
    let scratch = Scratch::new();
    let path = scratch.0.join(FILE);
    let newer = br#"{"schema":3,"games":{"uuid:a":{"prior":{"mouse_gain":[1.5,1.5],
        "mouse_lag_us":32000,"picture_width":1920,"picture_height":1080,"future":1},
        "used":1000}}}"#;
    std::fs::write(&path, newer).unwrap();
    assert_eq!(scratch.store(Some("uuid:a")).load(), Some(prior(1.5)));
    scratch.store(Some("uuid:b")).save(prior(2.0), false);
    assert_eq!(std::fs::read(&path).unwrap(), newer);
}
