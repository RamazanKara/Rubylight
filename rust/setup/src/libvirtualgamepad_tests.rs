use super::*;
use crate::libvirtualdisplay::View;
use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Default)]
struct Fake {
    devices: Vec<(String, Vec<String>)>,
    infs: Vec<(String, String)>,
    codes: HashMap<String, i32>,
    log: Vec<String>,
}

impl Windows for Fake {
    fn devices(&mut self, hardware_id: &str) -> Result<Vec<String>> {
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
    fn layers(&mut self, _view: View) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
    fn delete_layer(&mut self, _view: View, _name: &str) -> Result<()> {
        Ok(())
    }
}

#[test]
fn removes_only_legacy_gamepad_device_and_packages_leaving_rubylight() {
    let mut windows = Fake {
        devices: vec![
            (
                r"ROOT\VIBESHINEVIRTUALGAMEPAD\0000".into(),
                vec![r"Root\VibeshineVirtualGamepad".into()],
            ),
            (
                r"ROOT\RUBYLIGHTVIRTUALGAMEPAD\0000".into(),
                vec![r"Root\RubylightVirtualGamepad".into()],
            ),
        ],
        infs: vec![
            (
                "oem9.inf".into(),
                "[Version]\r\nCatalogFile = VibeshineVhfGamepad.cat\r\n".into(),
            ),
            (
                "oem15.inf".into(),
                "[Version]\r\nCatalogFile = rubylight_vgamepad.cat\r\n".into(),
            ),
        ],
        ..Default::default()
    };

    let removal = remove(&mut windows);
    assert!(removal.error.is_none(), "{:?}", removal.error);
    assert!(!removal.restart);
    assert_eq!(
        windows.log,
        [
            r"pnputil /remove-device ROOT\VIBESHINEVIRTUALGAMEPAD\0000",
            "pnputil /delete-driver oem9.inf /uninstall /force",
        ]
    );
}

#[test]
fn records_restart_when_pnputil_returns_3010_and_stops_on_error() {
    let mut windows = Fake {
        devices: vec![(
            r"ROOT\VIBESHINEVIRTUALGAMEPAD\0000".into(),
            vec![r"ROOT\VIBESHINEVIRTUALGAMEPAD".into()],
        )],
        infs: vec![(
            "oem9.inf".into(),
            "[Version]\nCatalogFile=vibeshinevhfgamepad.cat\n".into(),
        )],
        codes: HashMap::from([
            (
                r"/remove-device ROOT\VIBESHINEVIRTUALGAMEPAD\0000".into(),
                3010,
            ),
            ("/delete-driver oem9.inf /uninstall /force".into(), 5),
        ]),
        ..Default::default()
    };

    let removal = remove(&mut windows);
    assert!(removal.restart);
    let err = removal.error.unwrap().to_string();
    assert!(err.contains("exit code 5"), "{err}");
    if let Err::<(), _>(e) = bail_example() {
        assert!(!e.to_string().is_empty());
    }
}

fn bail_example() -> Result<()> {
    bail!("ok")
}
