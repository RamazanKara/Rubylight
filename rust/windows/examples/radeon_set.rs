//! Sets Radeon 3D settings through the host's journaled controller and restores them,
//! for measurements that need a driver feature on (AFMF, B7). Every change is journaled
//! in JOURNAL_DIR, so `restore` (or the host's startup recovery) puts it back even after
//! a crash.
//!
//! usage: radeon_set JOURNAL_DIR set ID=VALUE [ID=VALUE ...]
//!        radeon_set JOURNAL_DIR restore
//!        radeon_set JOURNAL_DIR pending
//! VALUE is `true`/`false` for a toggle, a number for a range, anything else for a choice.
#![allow(clippy::print_stdout)]
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use butterpollo_windows::radeon::{
        AdlxBackend, Change, Controller, Scope, adlx, control::JOURNAL_FILE, model::Value,
    };
    let mut args = std::env::args().skip(1);
    let usage = "usage: radeon_set JOURNAL_DIR set ID=VALUE ...|restore|pending";
    let dir = std::path::PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&dir)?;
    let mut controller = Controller::new(AdlxBackend, dir.join(JOURNAL_FILE));
    match args.next().context(usage)?.as_str() {
        "set" => {
            let changes = args
                .map(|pair| {
                    let (id, value) = pair.split_once('=').context("expected ID=VALUE")?;
                    let value = match value {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        other => match other.parse::<i64>() {
                            Ok(number) => Value::Int(number),
                            Err(_) => Value::Choice(other.to_string()),
                        },
                    };
                    Ok(Change {
                        id: id.to_string(),
                        value,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if changes.is_empty() {
                bail!("{usage}");
            }
            let gpu = adlx::gpus()?.first().context("no AMD GPU")?.id;
            let readback = controller
                .apply("radeon_set", gpu, &changes)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            for change in &changes {
                let now = readback.iter().find(|s| s.id == change.id);
                println!(
                    "RADEON_SET {} requested={:?} read_back={:?}",
                    change.id,
                    change.value,
                    now.and_then(|s| s.value.clone())
                );
            }
        }
        "restore" => {
            let report = controller.restore(Scope::All)?;
            for outcome in &report.outcomes {
                println!("RADEON_SET restore {} {:?}", outcome.id, outcome.result);
            }
            if !report.is_clean() {
                bail!("some settings could not be restored; they stay journaled");
            }
        }
        "pending" => {
            for entry in controller.pending()? {
                println!("RADEON_SET pending {entry:?}");
            }
        }
        _ => bail!("{usage}"),
    }
    Ok(())
}
