use super::*;

#[test]
fn a_remote_monitors_repair_that_failed_says_so_on_the_launchs_card() {
    let warnings = Warnings::default();
    report_repair(&warnings, Ok(()));
    assert!(warnings.snapshot().is_empty());
    report_repair(
        &warnings,
        Err(anyhow::anyhow!("refused").context("putting it back")),
    );
    let card = warnings.snapshot();
    assert_eq!(card.len(), 1);
    assert_eq!(card[0].code, "display_monitor_repair");
    assert!(card[0].message.contains("putting it back: refused"));
}
