//! Nucleus Co-op's DPI setting, undone. As it starts each instance, Nucleus
//! Co-op 2.4.2 sets the instance's .exe to a DPI mode (`~ HIGHDPIAWARE`) in
//! the user's compatibility layers (`GameDpiAwareness.SetExeDpiAwareness`),
//! replacing the compatibility settings the user chose for that .exe, and
//! never puts them back. A session keeps its game's values from before
//! Nucleus starts and restores them exactly when it ends, however it ends.
//! They wait in a file beside Nucleus until then, so a host that stopped
//! mid-session restores them before the next session starts.
use crate::text::to_wide;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW,
    RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

/// The compatibility layers in a user's hive: one value per program path.
pub const KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// A registry value: its type and bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub kind: u32,
    pub data: Vec<u8>,
}

/// One user's compatibility layers: their hive on the PC, a map in tests.
pub trait Layers: Send {
    /// The user's SID.
    fn user(&self) -> &str;
    /// Every value by name; none without the key.
    fn values(&mut self) -> Result<BTreeMap<String, Value>>;
    fn set(&mut self, name: &str, value: &Value) -> Result<()>;
    /// Remove a value; one that is gone already is fine.
    fn delete(&mut self, name: &str) -> Result<()>;
}

/// The values of a game's .exe before Nucleus started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Whose layers they are.
    pub user: String,
    /// The .exe's file name: Nucleus sets the value of each instance's path
    /// (the game's own, or its links in Nucleus' content folder).
    pub executable: String,
    pub values: BTreeMap<String, Value>,
}

/// Whether value `name`, a program's path, is one of `executable`.
fn of(name: &str, executable: &str) -> bool {
    let file = |path: &str| path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    file(name.trim()).eq_ignore_ascii_case(&file(executable.trim()))
}

/// `executable`'s values in `layers` now.
pub fn snapshot(layers: &mut dyn Layers, executable: &str) -> Result<Snapshot> {
    let values = layers
        .values()?
        .into_iter()
        .filter(|(name, _)| of(name, executable))
        .collect();
    Ok(Snapshot {
        user: layers.user().to_string(),
        executable: executable.to_string(),
        values,
    })
}

/// Put `snapshot` back: a value of its .exe that changed gets its old data,
/// one added since goes, one removed since returns. Returns how many values
/// it changed.
pub fn restore(layers: &mut dyn Layers, snapshot: &Snapshot) -> Result<usize> {
    let now = layers.values()?;
    let mut changed = 0;
    for (name, value) in now
        .iter()
        .filter(|(name, _)| of(name, &snapshot.executable))
    {
        match snapshot.values.get(name) {
            Some(old) if old == value => continue,
            Some(old) => layers.set(name, old)?,
            None => layers.delete(name)?,
        }
        changed += 1;
    }
    for (name, old) in &snapshot.values {
        if !now.contains_key(name) {
            layers.set(name, old)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// The file in Nucleus' `folder` that keeps `user`'s values until they are
/// restored.
fn journal(folder: &Path, user: &str) -> PathBuf {
    folder.join(format!("Rubylight-layers-{user}.json"))
}

/// A session's values, restored once when it is dropped or
/// [`restore`](Guard::restore) is called.
pub struct Guard {
    layers: Box<dyn Layers>,
    snapshot: Snapshot,
    journal: PathBuf,
    restored: bool,
}

impl Guard {
    /// Before Nucleus Co-op at `folder` starts `executable`: restore what a
    /// session the host never ended left, then keep the values now.
    pub fn take(mut layers: Box<dyn Layers>, folder: &Path, executable: &str) -> Result<Self> {
        let journal = journal(folder, layers.user());
        restore_left(layers.as_mut(), &journal)?;
        let snapshot = snapshot(layers.as_mut(), executable)?;
        let text = serde_json::to_vec_pretty(&snapshot)?;
        std::fs::write(&journal, text).with_context(|| format!("writing {}", journal.display()))?;
        Ok(Self {
            layers,
            snapshot,
            journal,
            restored: false,
        })
    }

    /// Put the values back, once. Should that fail, they stay in the file
    /// for the next session.
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.restored, true) {
            return;
        }
        match restore(self.layers.as_mut(), &self.snapshot) {
            Ok(changed) => {
                tracing::info!(
                    executable = self.snapshot.executable,
                    changed,
                    "Nucleus Co-op: the game's compatibility settings are back as they were"
                );
                if let Err(error) = std::fs::remove_file(&self.journal) {
                    tracing::warn!(
                        %error,
                        file = %self.journal.display(),
                        "Nucleus Co-op: the restored compatibility settings' file was not removed"
                    );
                }
            }
            Err(error) => tracing::error!(
                error = format!("{error:#}"),
                file = %self.journal.display(),
                "Nucleus Co-op: the game's compatibility settings were not put back; the next session tries again"
            ),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Restore the values in `journal`, which a session the host never ended
/// left, and remove it. A session must not start before: its own snapshot
/// would keep Nucleus' values as the user's.
fn restore_left(layers: &mut dyn Layers, journal: &Path) -> Result<()> {
    let text = match std::fs::read(journal) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", journal.display())),
    };
    let left = serde_json::from_slice::<Snapshot>(&text)
        .with_context(|| format!("reading {}", journal.display()))
        .and_then(|left| restore(layers, &left).map(|changed| (left, changed)))
        .with_context(|| {
            format!(
                "the compatibility settings an earlier Nucleus Co-op session changed were not put back from {}",
                journal.display()
            )
        })?;
    tracing::info!(
        executable = left.0.executable,
        changed = left.1,
        "Nucleus Co-op: an earlier session's compatibility settings are back as they were"
    );
    std::fs::remove_file(journal).with_context(|| format!("removing {}", journal.display()))
}

/// The signed-in user's layers in their hive (`HKEY_USERS\<SID>`), which
/// Nucleus Co-op, running as them, sees as its own.
pub struct UserHive {
    sid: String,
    path: Vec<u16>,
}

impl UserHive {
    pub fn signed_in() -> Result<Self> {
        let sid = crate::process::user_sid().context(
            "finding the signed-in user, whose compatibility settings Nucleus Co-op changes",
        )?;
        let path = to_wide(&format!(r"{sid}\{KEY}"));
        Ok(Self { sid, path })
    }

    /// The key opened with `access`; None when it is not there.
    fn open(&self, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<Option<Key>> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once opened.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                Some(0),
                access,
                &mut key,
            )
        };
        if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
            return Ok(None);
        }
        opened.ok().with_context(|| self.describe("opening"))?;
        Ok(Some(Key(key)))
    }

    fn describe(&self, what: &str) -> String {
        format!(r"{what} HKEY_USERS\{}\{KEY}", self.sid)
    }
}

/// An open registry key, closed on drop.
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

impl Layers for UserHive {
    fn user(&self) -> &str {
        &self.sid
    }

    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        let Some(key) = self.open(KEY_QUERY_VALUE)? else {
            return Ok(BTreeMap::new());
        };
        read_values(&key).with_context(|| self.describe("reading"))
    }

    fn set(&mut self, name: &str, value: &Value) -> Result<()> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once created.
        unsafe {
            RegCreateKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        }
        .ok()
        .with_context(|| self.describe("opening"))?;
        let key = Key(key);
        let wide = to_wide(name);
        // SAFETY: the name is terminated and the data a live slice.
        unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(wide.as_ptr()),
                None,
                REG_VALUE_TYPE(value.kind),
                Some(&value.data),
            )
        }
        .ok()
        .with_context(|| self.describe(&format!("setting {name} in")))
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        let Some(key) = self.open(KEY_SET_VALUE)? else {
            return Ok(());
        };
        let wide = to_wide(name);
        // SAFETY: the key is open and the name terminated.
        let deleted = unsafe { RegDeleteValueW(key.0, PCWSTR(wide.as_ptr())) };
        if deleted == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        deleted
            .ok()
            .with_context(|| self.describe(&format!("removing {name} from")))
    }
}

/// Every value of `key`. A value that grew while it was read is read again
/// with room for it.
fn read_values(key: &Key) -> Result<BTreeMap<String, Value>> {
    let (mut name_chars, mut data_bytes) = (0u32, 0u32);
    // SAFETY: the key is open; the counts are out-pointers.
    unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut name_chars),
            Some(&mut data_bytes),
            None,
            None,
        )
    }
    .ok()?;
    let mut name = vec![0u16; name_chars as usize + 1];
    let mut data = vec![0u8; (data_bytes as usize).max(1)];
    let mut values = BTreeMap::new();
    let mut index = 0;
    let mut retries = 0;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: the key is open, and the buffers and their lengths are
        // live for the call.
        let result = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            return Ok(values);
        }
        if result == ERROR_MORE_DATA && retries < 8 {
            retries += 1;
            name.resize(name.len() * 2, 0);
            data.resize((data_len as usize).max(data.len() * 2), 0);
            continue;
        }
        result.ok()?;
        values.insert(
            String::from_utf16_lossy(&name[..name_len as usize]),
            Value {
                kind,
                data: data[..data_len as usize].to_vec(),
            },
        );
        index += 1;
        retries = 0;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Layers in a map, shared with the test; `broken` fails every call.
    #[derive(Clone, Default)]
    pub struct Map {
        pub values: Arc<Mutex<BTreeMap<String, Value>>>,
        pub broken: Arc<Mutex<bool>>,
    }
    impl Map {
        fn check(&self) -> Result<()> {
            anyhow::ensure!(!*self.broken.lock().unwrap(), "access denied");
            Ok(())
        }
    }
    impl Layers for Map {
        fn user(&self) -> &str {
            "S-1-5-21-7"
        }
        fn values(&mut self) -> Result<BTreeMap<String, Value>> {
            self.check()?;
            Ok(self.values.lock().unwrap().clone())
        }
        fn set(&mut self, name: &str, value: &Value) -> Result<()> {
            self.check()?;
            self.values
                .lock()
                .unwrap()
                .insert(name.into(), value.clone());
            Ok(())
        }
        fn delete(&mut self, name: &str) -> Result<()> {
            self.check()?;
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }

    /// REG_SZ `text`, as the registry keeps it: UTF-16 and terminated.
    pub fn text(text: &str) -> Value {
        Value {
            kind: 1,
            data: to_wide(text)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }

    const GAME: &str = r"D:\Games\Puzzle\Puzzle.exe";

    /// What Nucleus does as it starts the instances: set each one's DPI mode.
    fn nucleus_runs(map: &Map, paths: &[&str]) {
        for path in paths {
            map.values
                .lock()
                .unwrap()
                .insert(path.to_string(), text("~ HIGHDPIAWARE"));
        }
    }

    #[test]
    fn the_users_settings_come_back_exactly_and_nucleus_values_go() {
        let map = Map::default();
        let other = r"C:\Tools\Other.exe";
        let before = BTreeMap::from([
            (GAME.to_string(), text("~ RUNASADMIN WIN7RTM")),
            (other.to_string(), text("~ DPIUNAWARE")),
            (
                r"E:\Old\Puzzle.exe".to_string(),
                Value {
                    kind: 2,
                    data: vec![1, 2, 3],
                },
            ),
        ]);
        *map.values.lock().unwrap() = before.clone();
        let folder = tempfile::tempdir().unwrap();
        let mut guard = Guard::take(Box::new(map.clone()), folder.path(), "puzzle.EXE").unwrap();
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        // Nucleus replaces the game's flags, sets its links' and one that
        // was gone; the user changes another program meanwhile.
        map.values.lock().unwrap().remove(r"E:\Old\Puzzle.exe");
        nucleus_runs(
            &map,
            &[
                GAME,
                r"C:\Nucleus\content\Rubylight-puzzle\Instance0\Puzzle.exe",
            ],
        );
        map.values
            .lock()
            .unwrap()
            .insert(other.into(), text("~ HIGHDPIAWARE"));
        guard.restore();
        let mut expected = before;
        expected.insert(other.into(), text("~ HIGHDPIAWARE"));
        assert_eq!(*map.values.lock().unwrap(), expected);
        assert!(!journal.exists());
        // Once only: a later change is the user's.
        nucleus_runs(&map, &[GAME]);
        drop(guard);
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ HIGHDPIAWARE"));
    }

    #[test]
    fn a_game_without_settings_has_none_again_and_a_dropped_guard_restores() {
        let map = Map::default();
        let folder = tempfile::tempdir().unwrap();
        let guard = Guard::take(Box::new(map.clone()), folder.path(), "Puzzle.exe").unwrap();
        nucleus_runs(&map, &[GAME]);
        // A session that failed to start, or a thread that never ran.
        drop(guard);
        assert!(map.values.lock().unwrap().is_empty());
        assert!(!journal(folder.path(), "S-1-5-21-7").exists());
    }

    #[test]
    fn values_left_by_a_host_that_stopped_come_back_before_the_next_session() {
        let map = Map::default();
        map.values
            .lock()
            .unwrap()
            .insert(GAME.into(), text("~ WIN8RTM"));
        let folder = tempfile::tempdir().unwrap();
        let guard = Guard::take(Box::new(map.clone()), folder.path(), "Puzzle.exe").unwrap();
        nucleus_runs(&map, &[GAME]);
        // The host stops before the session ends.
        std::mem::forget(guard);
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        // The next session, of another game, restores first, then keeps its own.
        let next = Guard::take(Box::new(map.clone()), folder.path(), "Other.exe").unwrap();
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
        let kept: Snapshot = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        assert_eq!(kept.executable, "Other.exe");
        drop(next);
        assert!(!journal.exists());
    }

    #[test]
    fn values_that_cannot_be_restored_are_kept_and_block_the_next_session() {
        let map = Map::default();
        map.values
            .lock()
            .unwrap()
            .insert(GAME.into(), text("~ WIN8RTM"));
        let folder = tempfile::tempdir().unwrap();
        let guard = Guard::take(Box::new(map.clone()), folder.path(), "Puzzle.exe").unwrap();
        nucleus_runs(&map, &[GAME]);
        *map.broken.lock().unwrap() = true;
        drop(guard);
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        // No session may snapshot Nucleus' value as the user's.
        let error = Guard::take(Box::new(map.clone()), folder.path(), "Puzzle.exe")
            .err()
            .unwrap();
        let error = format!("{error:#}");
        assert!(error.contains("were not put back from"), "{error}");
        assert!(journal.is_file());
        *map.broken.lock().unwrap() = false;
        drop(Guard::take(Box::new(map.clone()), folder.path(), "Puzzle.exe").unwrap());
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
        assert!(!journal.exists());
    }

    #[test]
    fn only_the_games_exe_counts() {
        assert!(of(r"D:\A\Game.exe", "game.exe"));
        assert!(of(r"D:/A/Game.exe", r"bin\Game.exe"));
        assert!(!of(r"D:\A\MyGame.exe", "Game.exe"));
        assert!(!of(r"D:\Game.exe\Other.exe", "Game.exe"));
    }

    /// The signed-in user's real layers, read and a value set and removed
    /// again.
    #[test]
    #[ignore = "writes to the signed-in user's registry"]
    fn the_users_hive_reads_sets_and_removes() -> Result<()> {
        let mut hive = UserHive::signed_in()?;
        let name = r"C:\Rubylight\layers-test\Game.exe";
        let before = hive.values()?;
        hive.set(name, &text("~ HIGHDPIAWARE"))?;
        assert_eq!(hive.values()?[name], text("~ HIGHDPIAWARE"));
        hive.delete(name)?;
        hive.delete(name)?;
        assert_eq!(hive.values()?, before);
        Ok(())
    }
}
