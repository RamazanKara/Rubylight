//! Client builds with a known problem against this host.
//!
//! The console's device list shows the warning beside a device whose last
//! request carried a matching `User-Agent`. It never refuses a client: the
//! device pairs, launches and streams as before.

pub struct BrokenClient {
    /// The product token of the `User-Agent`, compared without case.
    pub product: &'static str,
    /// The first affected version.
    pub from: (u32, u32, u32),
    /// The first fixed version.
    pub fixed: (u32, u32, u32),
    /// What goes wrong, and what to update to.
    pub problem: &'static str,
    pub link: &'static str,
}

/// Empty until a release note or issue of ours names a broken client build.
pub const BROKEN_CLIENTS: &[BrokenClient] = &[];

pub fn warning(user_agent: &str) -> Option<&'static BrokenClient> {
    warning_in(BROKEN_CLIENTS, user_agent)
}

pub fn warning_in<'a>(list: &'a [BrokenClient], user_agent: &str) -> Option<&'a BrokenClient> {
    user_agent.split_whitespace().find_map(|token| {
        let (product, version) = token.split_once('/')?;
        let version = version_of(version)?;
        list.iter().find(|client| {
            client.product.eq_ignore_ascii_case(product)
                && client.from <= version
                && version < client.fixed
        })
    })
}

/// `3.2.0`, `3.2` or `3.2.0-rc.1` as numbers. A pre-release counts as its
/// release, so a broken range that ends at `3.2.0` does not cover `3.2.0-rc.1`;
/// list such builds by their own range.
fn version_of(text: &str) -> Option<(u32, u32, u32)> {
    let core = text.split(['-', '+', ' ', '(']).next()?;
    let mut parts = core.split('.').map(str::parse::<u32>);
    let major = parts.next()?.ok()?;
    let minor = parts.next().transpose().ok()?.unwrap_or(0);
    let patch = parts.next().transpose().ok()?.unwrap_or(0);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &[BrokenClient] = &[BrokenClient {
        product: "Nightjar",
        from: (3, 1, 0),
        fixed: (3, 1, 4),
        problem: "freezes after a keyframe request",
        link: "https://example.invalid/issue",
    }];

    #[test]
    fn a_listed_build_is_warned_about() {
        for agent in [
            "Nightjar/3.1.0",
            "nightjar/3.1.3 (Android)",
            "Mozilla/5.0 Nightjar/3.1.2",
            "Nightjar/3.1",
        ] {
            assert!(warning_in(LIST, agent).is_some(), "{agent}");
        }
    }

    #[test]
    fn builds_outside_the_range_and_unknown_versions_are_not() {
        for agent in [
            "Nightjar/3.0.9",
            "Nightjar/3.1.4",
            "Nightjar/4.0.0",
            // Nightjar's streaming requests name no version today.
            "Nightjar",
            "Nightjar/",
            "Nightjar/x.1.2",
            "Moonlight/3.1.2",
            "okhttp/3.1.2",
            "",
        ] {
            assert!(warning_in(LIST, agent).is_none(), "{agent}");
        }
    }

    #[test]
    fn pre_releases_count_as_their_release() {
        assert!(warning_in(LIST, "Nightjar/3.1.4-rc.1").is_none());
        assert!(warning_in(LIST, "Nightjar/3.1.0-rc.1").is_some());
    }

    #[test]
    fn the_shipped_list_names_no_client_without_evidence() {
        // Each entry needs a release note or issue of ours as its link.
        for client in BROKEN_CLIENTS {
            assert!(client.link.starts_with("https://github.com/RamazanKara/"));
            assert!(client.from < client.fixed);
        }
        assert!(warning("Nightjar/3.2.0").is_none());
    }
}
