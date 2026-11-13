//! Copy an existing Windows profile into an empty, independent Rust profile.
use crate::{catalog, config::Config, crypto, state};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}
const FILE_LIMIT: u64 = 64 * 1024 * 1024;
/// Copy a file the host needs. Leaving it out would lose paired devices,
/// apps or credentials, so a link or an oversized file fails the import.
fn copy_file(source: &Path, destination: &Path, files: &mut usize, total: &mut u64) -> Result<()> {
    let metadata = source.symlink_metadata()?;
    if linked(&metadata) {
        bail!("profile file is a link: {}", source.display());
    }
    if metadata.len() > FILE_LIMIT {
        bail!("profile file is larger than 64 MiB: {}", source.display());
    }
    *files += 1;
    *total = total.saturating_add(metadata.len());
    std::fs::create_dir_all(
        destination
            .parent()
            .context("profile file needs a directory")?,
    )?;
    std::fs::copy(source, destination)?;
    Ok(())
}
/// Copy a file the host can do without. A link, a file over 64 MiB, or one
/// past 10,000 files or 512 MiB in all is left out with a warning instead of
/// failing the import. Returns whether the file was copied.
fn copy_optional(
    source: &Path,
    destination: &Path,
    files: &mut usize,
    total: &mut u64,
) -> Result<bool> {
    let metadata = source.symlink_metadata()?;
    let reason = if linked(&metadata) {
        "it is a link"
    } else if metadata.len() > FILE_LIMIT {
        "it is larger than 64 MiB"
    } else if *files >= 10000 || total.saturating_add(metadata.len()) > 512 * 1024 * 1024 {
        "the import is limited to 10,000 files and 512 MiB"
    } else {
        copy_file(source, destination, files, total)?;
        return Ok(true);
    };
    tracing::warn!(file = %source.display(), reason, "profile file not imported");
    Ok(false)
}
pub fn profile_id(directory: &Path) -> String {
    let canonical = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    hex::encode(Sha256::digest(
        canonical.to_string_lossy().to_lowercase().as_bytes(),
    ))
}
/// Copy a profile folder. The previous host's logs folder stays behind: its
/// session logs can fill hundreds of MiB, and the new host writes its own.
fn copy_tree(
    source: &Path,
    destination: &Path,
    files: &mut usize,
    total: &mut u64,
    root: bool,
) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let metadata = entry.path().symlink_metadata()?;
        if linked(&metadata) || metadata.is_file() {
            copy_optional(
                &entry.path(),
                &destination.join(entry.file_name()),
                files,
                total,
            )?;
        } else if metadata.is_dir() && !(root && entry.file_name().eq_ignore_ascii_case("logs")) {
            copy_tree(
                &entry.path(),
                &destination.join(entry.file_name()),
                files,
                total,
                false,
            )?;
        }
    }
    Ok(())
}
pub fn import(source: &Path, destination: &Path) -> Result<()> {
    let source = source
        .canonicalize()
        .context("existing profile folder unavailable")?;
    let config = Config::load(&source.join("sunshine.conf"))?;
    if !source.join("sunshine.conf").is_file() {
        bail!("select the folder containing sunshine.conf");
    }
    for path in [
        config.path("file_state", &source, "sunshine_state.json"),
        config.path("file_apps", &source, "apps.json"),
    ] {
        match path.symlink_metadata() {
            Ok(metadata) => {
                if linked(&metadata) {
                    bail!("profile file is a link: {}", path.display());
                }
                if metadata.len() > FILE_LIMIT {
                    bail!("profile file is larger than 64 MiB: {}", path.display());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Validate familiar documents before committing an import; unknown fields stay.
    let paired =
        state::PairedState::load(&config.path("file_state", &source, "sunshine_state.json"))?;
    state::load_json(
        &config.path("file_apps", &source, "apps.json"),
        serde_json::json!({}),
    )?;
    if destination.exists() && std::fs::read_dir(destination)?.next().is_some() {
        bail!("destination already contains a profile");
    }
    let parent = destination
        .parent()
        .context("profile needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let destination = parent.canonicalize()?.join(
        destination
            .file_name()
            .context("invalid profile directory")?,
    );
    if destination.starts_with(&source) || source.starts_with(&destination) {
        bail!("profiles must be in independent folders");
    }
    let stage = destination.parent().unwrap().join(format!(
        ".butterpollo-import-{}",
        hex::encode(crate::crypto::random::<16>())
    ));
    let result = (|| -> Result<()> {
        let (mut files, mut total) = (0, 0);
        copy_tree(&source, &stage, &mut files, &mut total, true)?;
        let mut rewritten = config.clone();
        // The certificate and key only work as a pair. When one is missing
        // the C++ host makes a new pair; clearing the default place lets this
        // host do the same instead of refusing to start.
        let identity = [
            config.path("cert", &source, "credentials/cacert.pem"),
            config.path("pkey", &source, "credentials/cakey.pem"),
        ];
        let identity_complete = identity.iter().all(|path| path.is_file());
        if !identity_complete && !paired.clients.is_empty() {
            bail!(
                "The paired profile's certificate or private key is missing. Restore both identity files before importing so existing clients can still connect."
            );
        }
        if !identity_complete {
            if identity.iter().any(|path| path.is_file())
                || config.values.contains_key("cert")
                || config.values.contains_key("pkey")
            {
                tracing::warn!(
                    certificate = %identity[0].display(),
                    key = %identity[1].display(),
                    "the certificate or its key is missing; the host makes a new pair"
                );
            }
            for target in ["credentials/cacert.pem", "credentials/cakey.pem"] {
                if stage.join(target).is_file() {
                    std::fs::remove_file(stage.join(target))?;
                }
            }
        }
        for (key, target) in [
            ("file_state", "sunshine_state.json"),
            ("file_apps", "apps.json"),
            ("vibeshine_file_state", "vibeshine_state.json"),
            ("credentials_file", "sunshine_credentials.json"),
            ("cert", "credentials/cacert.pem"),
            ("pkey", "credentials/cakey.pem"),
        ] {
            let configured = config.values.contains_key(key);
            let original = config.path(key, &source, target);
            let usable = if matches!(key, "cert" | "pkey") {
                identity_complete
            } else {
                original.is_file()
            };
            if usable && (configured || !stage.join(target).is_file()) {
                // The folder copy leaves out what is past its limits; these
                // files are needed, so they are copied in any case.
                let path = stage.join(target);
                copy_file(&original, &path, &mut files, &mut total)?;
                if configured {
                    rewritten.values.insert(key.into(), target.into());
                }
            } else if usable || !configured {
                continue;
            } else if key == "vibeshine_file_state" {
                rewritten.values.insert(key.into(), target.into());
            } else {
                // As in the C++ host, the default file is used (or made).
                if !matches!(key, "cert" | "pkey") {
                    tracing::warn!(
                        key,
                        file = %original.display(),
                        "a configured profile file is missing; the default is used"
                    );
                }
                rewritten.values.remove(key);
            }
        }
        // Own existing PNG covers too, so uninstalling the original host does
        // not remove the migrated library's artwork. Commands remain intact.
        let apps_path = rewritten.path("file_apps", &stage, "apps.json");
        if apps_path.is_file() {
            let mut apps = state::load_json(&apps_path, serde_json::json!({}))?;
            let mut changed = false;
            if let Some(entries) = apps
                .get_mut("apps")
                .and_then(serde_json::Value::as_array_mut)
            {
                for app in entries {
                    let Some(image) = app.get("image-path").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    let image = PathBuf::from(image);
                    let original = if image.is_absolute() {
                        image
                    } else {
                        source.join(image)
                    };
                    if !original.is_file()
                        || !original
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                    {
                        continue;
                    }
                    // A cover that is not copied keeps its path; the app
                    // shows the default artwork if the host cannot use it.
                    let metadata = original.symlink_metadata()?;
                    if linked(&metadata) || metadata.len() > 16 * 1024 * 1024 {
                        tracing::warn!(
                            cover = %original.display(),
                            "cover not imported: it is a link or larger than 16 MiB"
                        );
                        continue;
                    }
                    let bytes = std::fs::read(&original)?;
                    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                        continue;
                    }
                    let target = PathBuf::from("covers")
                        .join(format!("{}.png", hex::encode(Sha256::digest(&bytes))));
                    if !stage.join(&target).is_file()
                        && !copy_optional(&original, &stage.join(&target), &mut files, &mut total)?
                    {
                        continue;
                    }
                    app["image-path"] =
                        serde_json::json!(destination.join(&target).to_string_lossy());
                    changed = true;
                }
            }
            if changed {
                state::atomic_write(&apps_path, &serde_json::to_vec_pretty(&apps)?)?;
            }
        }
        // Log output must also stay in the newly owned profile.
        if config.values.contains_key("log_path") {
            rewritten
                .values
                .insert("log_path".into(), "butterpollo.log".into());
        }
        state::atomic_write(&stage.join("sunshine.conf"), rewritten.text().as_bytes())?;
        check(&stage).context("the imported settings would keep Rubylight from starting")?;
        if destination.exists() {
            std::fs::remove_dir(&destination)?;
        }
        std::fs::rename(&stage, &destination)?;
        Ok(())
    })();
    if result.is_err() && stage.exists() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}
/// Read a profile as the host does when it starts, without writing to it,
/// so that a file the host would refuse fails the import instead.
fn check(directory: &Path) -> Result<()> {
    let config = Config::load(&directory.join("sunshine.conf"))?;
    let files = state::ProfileFiles::new(&config, directory);
    state::PairedState::load(&files.paired)?;
    crypto::Identity::read(&files.certificate, &files.key)?;
    state::Credentials::load(&files.credentials)?;
    let mut aliases = state::load_json(&files.aliases, json!({"root":{}}))?;
    // Setup may remove the original host after import, so an unreadable
    // library must fail here rather than start the new host with no apps.
    let library = state::load_json(&files.apps, json!({}))?;
    let mut apps: Vec<state::App> =
        serde_json::from_value(library.get("apps").cloned().unwrap_or(json!([])))
            .context("invalid app library")?;
    for app in &mut apps {
        if app
            .extra
            .get("uuid")
            .and_then(Value::as_str)
            .is_none_or(|uuid| uuid.trim().is_empty())
        {
            app.extra
                .insert("uuid".into(), json!(uuid::Uuid::new_v4().to_string()));
        }
    }
    catalog::assign(&mut apps, &mut aliases, directory)
}
#[cfg(test)]
mod tests;
