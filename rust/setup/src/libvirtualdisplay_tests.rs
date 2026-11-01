use super::*;
use std::collections::HashMap;

/// Windows as a list of what is installed, logging every change asked.
#[derive(Default)]
struct Fake {
    devices: Vec<(String, Vec<String>)>,
    infs: Vec<(String, String)>,
    layers: HashMap<&'static str, Vec<String>>,
    /// pnputil's exit code by its arguments; 0 otherwise.
    codes: HashMap<String, i32>,
    unreadable_devices: bool,
    log: Vec<String>,
}

fn view_name(view: View) -> &'static str {
    match view {
        View::Native64 => "64",
        View::Wow32 => "32",
    }
}

impl Windows for Fake {
    fn devices(&mut self, hardware_id: &str) -> Result<Vec<String>> {
        if self.unreadable_devices {
            bail!("listing the devices: access denied");
        }
        Ok(self
            .devices
            .iter()
            .filter(|(_, ids)| ids.iter().any(|id| id.eq_ignore_ascii_case(hardware_id)))
            .map(|(instance, _)| instance.clone())
            .collect())
    }
    fn oem_infs(&mut self) -> Result<Vec<(String, String)>> {
        Ok(self.infs.clone())
    }
    fn pnputil(&mut self, args: &[&str]) -> Result<i32> {
        let args = args.join(" ");
        self.log.push(format!("pnputil {args}"));
        Ok(self.codes.get(&args).copied().unwrap_or(0))
    }
    fn layers(&mut self, view: View) -> Result<Option<Vec<String>>> {
        Ok(self.layers.get(view_name(view)).cloned())
    }
    fn delete_layer(&mut self, view: View, name: &str) -> Result<()> {
        self.log.push(format!("delete {} {name}", view_name(view)));
        Ok(())
    }
}

const OURS: &str = "[Version]\r\nSignature=\"$WINDOWS NT$\"\r\nClass=Display\r\n  CatalogFile = SunshineVirtualDisplayDriver.cat ; signed\r\n";

fn installed() -> Fake {
    Fake {
        devices: vec![
            (
                r"ROOT\DISPLAY\0001".into(),
                vec![r"ROOT\SUNSHINEVIRTUALDISPLAY".into()],
            ),
            (
                r"ROOT\RUBYLIGHTVDISPLAY\0000".into(),
                vec![r"Root\RubylightVDisplay".into()],
            ),
            (r"PCI\VEN_1002&DEV_744C\4&1".into(), Vec::new()),
        ],
        infs: vec![
            ("oem7.inf".into(), OURS.into()),
            (
                "oem12.inf".into(),
                "[Version]\nCatalogFile=rubylight_vdisplay.cat\n".into(),
            ),
            (
                "oem31.inf".into(),
                "[Version]\ncatalogfile=sunshinevirtualdisplaydriver.CAT\n".into(),
            ),
        ],
        layers: HashMap::from([
            (
                "64",
                vec![
                    r"C:\Program Files\Sunshine\VkLayer_sunshine_hdr.json".into(),
                    r"C:\Program Files\Rubylight\VkLayer_rubylight_hdr.json".into(),
                ],
            ),
            ("32", vec!["C:/Vibepollo/vklayer_SUNSHINE_hdr.json".into()]),
        ]),
        ..Default::default()
    }
}

#[test]
fn the_device_its_packages_and_its_layer_go_and_nothing_else() {
    let mut windows = installed();
    let removal = remove(&mut windows);
    assert!(removal.error.is_none(), "{:?}", removal.error);
    assert!(!removal.restart);
    assert_eq!(
        windows.log,
        [
            r"pnputil /remove-device ROOT\DISPLAY\0001",
            "pnputil /delete-driver oem7.inf /uninstall /force",
            "pnputil /delete-driver oem31.inf /uninstall /force",
            r"delete 64 C:\Program Files\Sunshine\VkLayer_sunshine_hdr.json",
            "delete 32 C:/Vibepollo/vklayer_SUNSHINE_hdr.json",
        ]
    );
}

#[test]
fn with_nothing_left_nothing_changes() {
    let mut windows = Fake::default();
    let removal = remove(&mut windows);
    assert!(removal.error.is_none() && !removal.restart);
    assert!(windows.log.is_empty(), "{:?}", windows.log);
}

#[test]
fn a_display_in_use_goes_at_the_next_restart() {
    let mut windows = installed();
    windows
        .codes
        .insert(r"/remove-device ROOT\DISPLAY\0001".into(), 3010);
    let removal = remove(&mut windows);
    assert!(removal.error.is_none(), "{:?}", removal.error);
    assert!(removal.restart);
    // The rest still goes.
    assert_eq!(windows.log.len(), 5, "{:?}", windows.log);
}

#[test]
fn a_failed_step_stops_the_removal_and_says_which() {
    let mut windows = installed();
    windows
        .codes
        .insert(r"/remove-device ROOT\DISPLAY\0001".into(), 3010);
    windows
        .codes
        .insert("/delete-driver oem7.inf /uninstall /force".into(), 5);
    let removal = remove(&mut windows);
    assert_eq!(
        removal.error.map(|error| format!("{error:#}")).as_deref(),
        Some("pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5")
    );
    // What went before still needs its restart.
    assert!(removal.restart);
    assert_eq!(windows.log.len(), 2, "{:?}", windows.log);
    // A device list Windows refuses is an error, never "nothing installed".
    let mut windows = installed();
    windows.unreadable_devices = true;
    let removal = remove(&mut windows);
    assert!(removal.error.is_some());
    assert!(windows.log.is_empty(), "{:?}", windows.log);
}

#[test]
fn the_catalog_line_is_matched_as_the_old_script_did() {
    assert!(names_catalog(OURS));
    assert!(names_catalog(
        "CatalogFile=SunshineVirtualDisplayDriver.cat"
    ));
    assert!(names_catalog(
        "\tCATALOGFILE\t=\tsunshinevirtualdisplaydriver.cat"
    ));
    // A platform's own catalog line, another driver, a comment.
    assert!(!names_catalog(
        "CatalogFile.NTamd64 = SunshineVirtualDisplayDriver.cat"
    ));
    assert!(!names_catalog("CatalogFile = rubylight_vdisplay.cat"));
    assert!(!names_catalog(
        "; CatalogFile = SunshineVirtualDisplayDriver.cat"
    ));
    assert!(!names_catalog(""));
}

#[test]
fn inf_text_is_read_in_its_encoding() {
    let line = "CatalogFile = SunshineVirtualDisplayDriver.cat\r\n";
    let mut le = vec![0xFF, 0xFE];
    le.extend(line.encode_utf16().flat_map(u16::to_le_bytes));
    let mut be = vec![0xFE, 0xFF];
    be.extend(line.encode_utf16().flat_map(u16::to_be_bytes));
    let mut utf8 = vec![0xEF, 0xBB, 0xBF];
    utf8.extend(line.bytes());
    for bytes in [le, be, utf8, line.as_bytes().to_vec()] {
        assert_eq!(inf_text(&bytes), line);
        assert!(names_catalog(&inf_text(&bytes)));
    }
}

#[test]
fn a_layers_file_name_is_its_last_component() {
    assert_eq!(
        file_name(r"C:\Program Files\Sunshine\VkLayer_sunshine_hdr.json"),
        "VkLayer_sunshine_hdr.json"
    );
    assert_eq!(
        file_name("C:/x/VkLayer_sunshine_hdr.json"),
        "VkLayer_sunshine_hdr.json"
    );
    assert_eq!(
        file_name("C:VkLayer_sunshine_hdr.json"),
        "VkLayer_sunshine_hdr.json"
    );
    assert_eq!(
        file_name("VkLayer_sunshine_hdr.json"),
        "VkLayer_sunshine_hdr.json"
    );
}

#[test]
fn this_pcs_devices_packages_and_layers_are_read() -> Result<()> {
    // Read-only: nothing is removed.
    let devices = all_devices()?;
    let (instance, ids) = devices
        .iter()
        .find(|(_, ids)| !ids.is_empty())
        .expect("a device with hardware ids");
    let found = Native.devices(&ids[0].to_ascii_lowercase())?;
    assert!(found.contains(instance), "{instance} not in {found:?}");
    println!(
        "libvirtualdisplay devices: {:?}",
        Native.devices(HARDWARE_ID)?
    );
    let infs = Native.oem_infs()?;
    assert!(
        infs.iter()
            .all(|(name, _)| name.to_ascii_lowercase().starts_with("oem")),
        "{infs:?}"
    );
    // Every driver store INF names some catalog or none; a decoding
    // mistake would show as text without its [Version] section.
    for (name, text) in &infs {
        assert!(text.to_ascii_lowercase().contains("[version]"), "{name}");
    }
    println!("libvirtualdisplay packages: {:?}", packages(&infs));
    for view in [View::Native64, View::Wow32] {
        println!("{view:?} layers: {:?}", Native.layers(view)?);
    }
    Ok(())
}
