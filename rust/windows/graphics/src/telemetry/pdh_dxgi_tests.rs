//! Parser, per-adapter aggregation and ADLX merge tests for the PDH collector; no GPU needed.
use super::*;

const A: &str = "luid_0x00000000_0x0000D1A4";
const B: &str = "luid_0x00000001_0x00013F22";

fn engine(pid: u32, luid: &str, eng: u32, engine_type: &str, value: f64) -> (String, f64) {
    (
        format!("pid_{pid}_{luid}_phys_0_eng_{eng}_engtype_{engine_type}"),
        value,
    )
}

fn adapter(luid: u64, name: &str) -> Adapter {
    Adapter {
        luid,
        name: name.into(),
        dedicated_bytes: 8 << 30,
    }
}

#[test]
fn instance_names_parse_luid_phys_engine_and_type() {
    let instance = parse_instance(
        "pid_1234_luid_0x00000001_0x0000D1A4_phys_0_eng_12_engtype_Video Codec Engine",
    )
    .unwrap();
    assert_eq!(instance.luid, (1 << 32) | 0xD1A4);
    assert_eq!(instance.phys, 0);
    assert_eq!(instance.engine, Some(12));
    assert_eq!(instance.engine_type, "Video Codec Engine");
    let memory = parse_instance("luid_0x00000000_0x00013F22_phys_1").unwrap();
    assert_eq!(
        (memory.luid, memory.phys, memory.engine),
        (0x13F22, 1, None)
    );
    assert_eq!(memory.engine_type, "");
    for bad in [
        "",
        "_Total",
        "pid_1_luid_0xZZ_0x1",
        "luid_0x1",
        "luid_0x1_0x",
        "luid_0x123456789_0x1",
    ] {
        assert!(parse_instance(bad).is_none(), "{bad}");
    }
}

#[test]
fn engine_types_classify_encode_and_codec_but_not_decode() {
    assert_eq!(kind("VideoEncode"), Some(Kind::Encoder));
    assert_eq!(kind("Video Encode 1"), Some(Kind::Encoder));
    assert_eq!(kind("Video Codec Engine"), Some(Kind::Encoder));
    assert_eq!(kind("3D"), Some(Kind::Render));
    for other in [
        "VideoDecode",
        "Video Decode 1",
        "Compute 0",
        "Copy",
        "Video JPEG 0",
        "Security 1",
        "",
    ] {
        assert_eq!(kind(other), None, "{other}");
    }
}

#[test]
fn adapters_are_never_summed_together_and_processes_add_up_per_engine() {
    let engines = vec![
        engine(1, A, 4, "VideoEncode", 30.),
        engine(2, A, 4, "VideoEncode", 25.),
        engine(3, A, 5, "VideoEncode", 40.),
        engine(1, B, 0, "Video Codec Engine", 10.),
        engine(1, A, 0, "3D", 70.),
        engine(2, A, 0, "3D", 50.),
        engine(1, B, 0, "3D", f64::NAN),
        engine(1, B, 1, "VideoDecode", 99.),
        ("_Total".into(), 100.),
    ];
    let memory = vec![
        (format!("{A}_phys_0"), 1024. * 1024. * 512.),
        (format!("{B}_phys_0"), -1.),
    ];
    let loads = aggregate(&engines, &memory);
    let a = loads[&0xD1A4];
    assert_eq!(a.encoder_percent, Some(55.));
    assert_eq!(a.render_percent, Some(100.));
    assert_eq!(a.dedicated_bytes, Some(1024. * 1024. * 512.));
    let b = loads[&((1 << 32) | 0x13F22)];
    assert_eq!(b.encoder_percent, Some(10.));
    assert_eq!(b.render_percent, None);
    assert_eq!(b.dedicated_bytes, None);
    assert_eq!(loads.len(), 2);
}

#[test]
fn first_sample_without_engine_readings_omits_rates() {
    let loads = aggregate(&[], &[(format!("{A}_phys_0"), 2048.)]);
    assert_eq!(loads[&0xD1A4].encoder_percent, None);
    let mut gpus = vec![];
    merge(
        &mut gpus,
        &[adapter(0xD1A4, "NVIDIA GeForce RTX 4080")],
        &loads,
        &[],
    );
    assert!(gpus.is_empty(), "no engine reading yet, so no sample");
}

#[test]
fn merge_attaches_to_adlx_and_synthesizes_other_adapters() {
    let mut gpus = vec![GpuSample {
        id: 7,
        name: "AMD Radeon RX 7900 XT".into(),
        metrics: BTreeMap::from([("gpu_usage_percent".into(), 12.)]),
    }];
    let adapters = [
        adapter(0xD1A4, "AMD Radeon RX 7900 XT"),
        adapter(0x13F22, "NVIDIA GeForce RTX 4080"),
        adapter(0x99, "Indirect Display"),
    ];
    let loads = BTreeMap::from([
        (
            0xD1A4,
            Load {
                encoder_percent: Some(33.),
                render_percent: Some(90.),
                dedicated_bytes: Some(1.),
            },
        ),
        (
            0x13F22,
            Load {
                encoder_percent: Some(20.),
                render_percent: Some(60.),
                dedicated_bytes: Some(2. * 1024. * 1024. * 1024.),
            },
        ),
        (0x99, Load::default()),
    ]);
    merge(&mut gpus, &adapters, &loads, &[]);
    assert_eq!(gpus.len(), 2);
    assert_eq!(
        gpus[0].metrics,
        BTreeMap::from([("gpu_usage_percent".into(), 12.), (ENCODER.into(), 33.)]),
        "ADLX readings stay; only the encoder load is added"
    );
    assert_eq!(gpus[1].id, synthetic_id(0x13F22));
    assert!(gpus[1].id < 0);
    assert_eq!(gpus[1].name, "NVIDIA GeForce RTX 4080");
    assert_eq!(
        gpus[1].metrics,
        BTreeMap::from([
            ("gpu_usage_percent".into(), 60.),
            (ENCODER.into(), 20.),
            ("gpu_vram_used_mb".into(), 2048.),
            ("gpu_vram_usage_percent".into(), 25.),
        ])
    );
    let mut selected = vec![];
    merge(&mut selected, &adapters[1..], &loads, &[ENCODER.into()]);
    assert_eq!(selected[0].metrics, BTreeMap::from([(ENCODER.into(), 20.)]));
}

#[test]
fn identical_cards_pair_one_to_one_in_order() {
    let adapters = [adapter(1, "Same"), adapter(2, "Same"), adapter(3, "Other")];
    assert_eq!(
        pair(&adapters, &["Same", "Same"]),
        vec![Some(0), Some(1), None]
    );
    assert_eq!(pair(&adapters, &["Same"]), vec![Some(0), None, None]);
    assert_ne!(synthetic_id(1), synthetic_id(2));
}
