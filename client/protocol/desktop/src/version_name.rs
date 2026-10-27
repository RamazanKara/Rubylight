//! Nightjar's release version is the Android `versionName` in `client/app/build.gradle`, the
//! line the release workflow checks the tag against. build.rs reads it with [`parse`] and
//! embeds it as `NIGHTJAR_VERSION`, so the desktop client reports the version Android does.

/// The value of the first `versionName "<version>"` line, matched like the release workflow's
/// `sed -n 's/^ *versionName "\(.*\)"$/\1/p'`. `None` when there is none or it is empty or has
/// anything but printable ASCII.
pub fn parse(gradle: &str) -> Option<&str> {
    let version = gradle.lines().find_map(|line| {
        line.trim_end_matches('\r')
            .trim_start_matches(' ')
            .strip_prefix("versionName \"")?
            .strip_suffix('"')
    })?;
    (!version.is_empty() && version.bytes().all(|b| b.is_ascii_graphic())).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn the_first_version_name_line_is_the_version() {
        let gradle = "android {\n    defaultConfig {\n        versionName \"3.2.3\"\n        versionCode = 357\n        versionName \"9.9.9\"\n    }\n}\n";
        assert_eq!(parse(gradle), Some("3.2.3"));
        assert_eq!(
            parse("  versionName \"3.0.0-rc.3\"\r\n"),
            Some("3.0.0-rc.3")
        );
    }

    #[test]
    fn a_missing_or_malformed_version_name_is_none() {
        assert_eq!(parse("versionCode = 357\n"), None);
        assert_eq!(parse("    versionName = \"3.2.3\"\n"), None);
        assert_eq!(parse("    // versionName \"3.2.3\"\n"), None);
        assert_eq!(parse("    versionName \"\"\n"), None);
        assert_eq!(parse("    versionName \"3.2 beta\"\n"), None);
    }

    #[test]
    fn the_client_reports_the_android_version_name() {
        let gradle = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/build.gradle"),
        )
        .expect("read app/build.gradle");
        assert_eq!(Some(crate::VERSION), parse(&gradle));
    }
}
