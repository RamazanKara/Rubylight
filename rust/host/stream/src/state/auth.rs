//! Host credential persistence/reloading, console web-session rotation, and app-library
//! save/reload operations.

use super::*;

impl Host {
    /// Write the app library and remember the file as written, so [`Host::reload_apps`] does
    /// not read the host's own save back. Call it with [`Host::apps`] write-locked.
    pub fn save_apps(&self, document: &Value) -> Result<()> {
        butterpollo_session::state::write_json(&self.apps_path, document)?;
        *self.apps_stamp.lock().unwrap() = file_stamp(&self.apps_path);
        Ok(())
    }

    /// Take up an app library another program changed on disk (ApolloSync, MoonDeckBuddy or a
    /// hand edit) before the apps are read or changed, so clients see it without a restart and
    /// the next save or sync builds on it instead of writing the old list back. A file that
    /// cannot be read as a library, or one that is gone, keeps the apps in memory.
    pub fn reload_apps(&self) {
        let current = file_stamp(&self.apps_path);
        if *self.apps_stamp.lock().unwrap() == current {
            return;
        }
        // The saves' lock order: apps, then the stamp.
        let mut apps = self.apps.write().unwrap();
        let current = file_stamp(&self.apps_path);
        if *self.apps_stamp.lock().unwrap() == current {
            return;
        }
        let loaded = (|| -> Result<(Value, Vec<App>)> {
            let bytes = std::fs::read(&self.apps_path)?;
            let json = butterpollo_session::state::json_bytes(&bytes)
                .context("the file is empty or not text")?;
            let mut document: Value = serde_json::from_slice(json)?;
            if normalize_library(&mut document) {
                self.save_apps(&document)?;
            }
            let mut next: Vec<App> =
                serde_json::from_value(document.get("apps").cloned().unwrap_or(json!([])))?;
            self.assign_apps(&mut next)?;
            Ok((document, next))
        })();
        match loaded {
            Ok((document, next)) => {
                *self.apps_stamp.lock().unwrap() = file_stamp(&self.apps_path);
                *apps = next;
                *self.app_document.write().unwrap() = document;
                self.config_version
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
                tracing::info!(apps = apps.len(), "took up the app library changed on disk");
            }
            Err(error) => tracing::warn!(
                error = %format!("{error:#}"),
                file = %self.apps_path.display(),
                "the app library changed on disk but cannot be read; the current apps stay"
            ),
        }
    }

    pub fn assign_apps(&self, apps: &mut [App]) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut next = aliases.clone();
        butterpollo_session::catalog::assign(
            apps,
            &mut next,
            self.assets.parent().unwrap_or(&self.assets),
        )?;
        butterpollo_session::state::write_json(&self.aliases_path, &next)?;
        *aliases = next;
        crate::host_warnings::network_covers(&self.warnings, apps);
        Ok(())
    }

    pub fn save_credentials(&self, credentials: &Credentials) -> Result<()> {
        let mut paired = self.paired.write().unwrap();
        let mut document =
            butterpollo_session::state::load_json(&self.credentials_path, json!({}))?;
        let values = serde_json::to_value(credentials)?;
        for (key, value) in values.as_object().unwrap() {
            document[key] = value.clone();
        }
        butterpollo_session::state::write_json(&self.credentials_path, &document)?;
        if self.credentials_path == self.paired_path {
            paired.document = document;
        }
        *self.credentials_stamp.lock().unwrap() = file_stamp(&self.credentials_path);
        Ok(())
    }

    /// Take up a sign-in changed on disk, as `--creds` changes it while the
    /// host runs, so the console accepts it at once. Apollo and Sunshine
    /// profiles keep the sign-in in the paired-state file; updating that
    /// copy too keeps the next save of the paired state from writing the
    /// previous sign-in back. Signed-in browsers sign in again, as after a
    /// password change in the console.
    pub fn reload_credentials(&self) {
        if *self.credentials_stamp.lock().unwrap() == file_stamp(&self.credentials_path) {
            return;
        }
        // The password change's order: sessions, paired state, sign-in.
        let mut sessions = self.web_sessions.lock().unwrap();
        {
            let mut paired = self.paired.write().unwrap();
            let mut seen = self.credentials_stamp.lock().unwrap();
            let current = file_stamp(&self.credentials_path);
            if *seen == current {
                return;
            }
            let loaded = match Credentials::load(&self.credentials_path) {
                Ok(loaded) => {
                    *seen = current;
                    loaded
                }
                Err(error) => {
                    tracing::warn!(
                        error = %format!("{error:#}"),
                        "the console sign-in file is unreadable; the current sign-in stays"
                    );
                    return;
                }
            };
            let mut credentials = self.credentials.write().unwrap();
            if *credentials == loaded {
                return;
            }
            if self.credentials_path == self.paired_path
                && let Some(document) = paired.document.as_object_mut()
            {
                for key in ["username", "password", "salt"] {
                    document.remove(key);
                }
                if let Some(Value::Object(values)) =
                    loaded.as_ref().and_then(|c| serde_json::to_value(c).ok())
                {
                    document.extend(values);
                }
            }
            *credentials = loaded;
            tracing::info!(
                "the console sign-in was changed outside the console; browsers sign in again"
            );
        }
        if let Err(error) = self.revoke_sign_ins() {
            tracing::warn!(error = %format!("{error:#}"), "browser sessions and API tokens could not be cleared");
        }
        sessions.clear();
        self.rotated_web_sessions.lock().unwrap().clear();
    }

    pub fn save_web_sessions(&self, sessions: &HashMap<String, WebSession>) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut document = aliases.clone();
        if !document["root"].is_object() {
            document["root"] = json!({});
        }
        document["root"]["session_tokens"] = Value::Array(
            sessions
                .iter()
                .filter(|(_, s)| s.refresh_expires > Instant::now())
                .map(|(key, s)| web_sessions::record(key, s))
                .collect(),
        );
        butterpollo_session::state::write_json(&self.aliases_path, &document)?;
        *aliases = document;
        self.rotated_web_sessions.lock().unwrap().clear();
        Ok(())
    }

    /// A changed console sign-in ends what the old one let in: browser
    /// sessions and API tokens, in one write. A token is bound only to the
    /// username, so it would otherwise outlive the password it was made with.
    pub fn revoke_sign_ins(&self) -> Result<()> {
        let mut aliases = self.aliases.lock().unwrap();
        let mut document = aliases.clone();
        if !document["root"].is_object() {
            document["root"] = json!({});
        }
        document["root"]["session_tokens"] = json!([]);
        document["root"]["api_tokens"] = json!([]);
        butterpollo_session::state::write_json(&self.aliases_path, &document)?;
        *aliases = document;
        self.rotated_web_sessions.lock().unwrap().clear();
        Ok(())
    }

    pub fn new_web_session(
        &self,
        username: String,
        remember_me: bool,
        user_agent: String,
        remote_address: String,
        previous: Option<WebSession>,
    ) -> Result<(String, String, String, u64)> {
        let ttl = self
            .config
            .read()
            .unwrap()
            .integer("session_token_ttl_seconds", 7200)
            .clamp(60, 604800) as u64;
        let access = hex::encode(butterpollo_crypto::crypto::random::<32>());
        let refresh = hex::encode(butterpollo_crypto::crypto::random::<32>());
        let csrf = hex::encode(butterpollo_crypto::crypto::random::<32>());
        let wall = web_sessions::now();
        let refresh_deadline = previous
            .as_ref()
            .map(|s| s.refresh_deadline)
            .unwrap_or_else(|| {
                wall + if remember_me {
                    self.config
                        .read()
                        .unwrap()
                        .integer("remember_me_refresh_token_ttl_seconds", 604800)
                        .clamp(60, 366 * 86400) as u64
                } else {
                    ttl.max(86400)
                }
            });
        let access_deadline = (wall + ttl).min(refresh_deadline);
        let mut extra = previous
            .as_ref()
            .map(|s| s.extra.clone())
            .unwrap_or_default();
        extra.insert(
            "rotation_id".into(),
            hex::encode(butterpollo_crypto::crypto::random::<12>()).into(),
        );
        const REFRESH_ROTATION_GRACE: Duration = Duration::from_secs(15);
        let mut sessions = self.web_sessions.lock().unwrap();
        let live = previous.as_ref().map(|previous| {
            sessions
                .values()
                .find(|s| s.refresh == previous.refresh && s.refresh_expires > Instant::now())
        });
        if let Some(None) = live {
            if let Some(prev) = previous.as_ref() {
                let mut rotated = self.rotated_web_sessions.lock().unwrap();
                rotated.retain(|_, entry| entry.rotated_at.elapsed() < REFRESH_ROTATION_GRACE);
                if let Some(entry) = rotated.get(&prev.refresh)
                    && sessions
                        .get(&entry.access_hash)
                        .is_some_and(|s| s.refresh_expires > Instant::now())
                {
                    return Ok((
                        entry.access.clone(),
                        entry.refresh.clone(),
                        entry.csrf.clone(),
                        entry.refresh_deadline.saturating_sub(wall),
                    ));
                }
            }
            return Err(web_sessions::Rotated.into());
        }
        // A refresh keeps the sign-in. It is read under the lock: the console's stream socket may
        // have given the live session its ID after `previous` was copied.
        let sign_in = live
            .flatten()
            .and_then(|s| s.extra.get(web_sessions::SIGN_IN_ID).cloned())
            .unwrap_or_else(|| hex::encode(butterpollo_crypto::crypto::random::<16>()).into());
        extra.insert(web_sessions::SIGN_IN_ID.into(), sign_in);
        let mut next = sessions.clone();
        next.retain(|_, s| s.refresh_expires > Instant::now());
        if let Some(previous) = previous.as_ref() {
            next.retain(|_, s| s.refresh != previous.refresh);
        }
        if next.len() >= 64
            && let Some(key) = next
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(key, _)| key.clone())
        {
            next.remove(&key);
        }
        let access_hash = web_sessions::hash(&access);
        next.insert(
            access_hash.clone(),
            WebSession {
                created: wall,
                last_seen: wall,
                refresh: web_sessions::hash(&refresh),
                csrf: csrf.clone(),
                expires: Instant::now() + Duration::from_secs(access_deadline.saturating_sub(wall)),
                refresh_expires: Instant::now()
                    + Duration::from_secs(refresh_deadline.saturating_sub(wall)),
                access_deadline,
                refresh_deadline,
                username,
                remember_me,
                user_agent,
                remote_address,
                extra,
            },
        );
        self.save_web_sessions(&next)?;
        *sessions = next;
        if let Some(prev) = previous.as_ref() {
            let mut rotated = self.rotated_web_sessions.lock().unwrap();
            rotated.retain(|_, entry| entry.rotated_at.elapsed() < REFRESH_ROTATION_GRACE);
            rotated.insert(
                prev.refresh.clone(),
                crate::state::RotatedWebSession {
                    rotated_at: Instant::now(),
                    access: access.clone(),
                    refresh: refresh.clone(),
                    csrf: csrf.clone(),
                    refresh_deadline,
                    access_hash,
                },
            );
        }
        Ok((access, refresh, csrf, refresh_deadline.saturating_sub(wall)))
    }
}
