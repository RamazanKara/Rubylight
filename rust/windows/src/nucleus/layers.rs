//! Nucleus Co-op's DPI setting, undone. As it starts each instance, Nucleus
//! Co-op 2.4.2 sets the instance's .exe to a DPI mode (`~ HIGHDPIAWARE`) in
//! the user's compatibility layers (`GameDpiAwareness.SetExeDpiAwareness`),
//! replacing the compatibility settings the user chose for that .exe, and
//! never puts them back. A session keeps its game's values from before
//! Nucleus starts and, when it ends however it ends, puts back each one
//! Nucleus' DPI mode still holds; a value the user changed meanwhile stays.
//! Only the game's own path and Nucleus' instance links count, never another
//! game's .exe of the same name. The values wait in a file beside Nucleus
//! until then, so what a host that stopped mid-session left comes back when
//! the service next sees the user sign in or starts a host, or before the
//! next session.
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

/// What Nucleus Co-op 2.4.2 sets an instance's .exe to
/// (`GameDpiAwareness.SetExeDpiAwareness`): a value with other data is not
/// Nucleus' (any more) and is never touched.
pub const NUCLEUS_DPI: &str = "~ HIGHDPIAWARE";

/// The values of a game's .exe before Nucleus started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Whose layers they are.
    pub user: String,
    /// The .exe's file name: Nucleus sets the value of each instance's path
    /// (the game's own, or its links in Nucleus' content folder).
    pub executable: String,
    /// The game's .exe as Nucleus starts it. None in a file an earlier build
    /// wrote, which counts every path of the .exe's file name.
    #[serde(default)]
    pub game: Option<String>,
    /// Nucleus' content folder, where it links each instance's files.
    #[serde(default)]
    pub content: Option<String>,
    pub values: BTreeMap<String, Value>,
}

/// A path as the registry compares value names: case aside, `\` only.
fn plain(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

/// Whether value `name`, a program's path, is one of `executable`.
fn of(name: &str, executable: &str) -> bool {
    let file = |path: &str| path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    file(name.trim()).eq_ignore_ascii_case(&file(executable.trim()))
}

impl Snapshot {
    /// Whether value `name` is one Nucleus sets for this session: the game's
    /// own path, or an instance's .exe below Nucleus' content folder.
    /// Another program with the same file name (every RPG Maker game is
    /// `Game.exe`) is not.
    fn covers(&self, name: &str) -> bool {
        if self.game.is_none() && self.content.is_none() {
            return of(name, &self.executable);
        }
        let name = plain(name);
        let game = self.game.as_deref().is_some_and(|game| plain(game) == name);
        let instance = self.content.as_deref().is_some_and(|content| {
            let content = plain(content);
            let content = content.trim_end_matches('\\');
            !content.is_empty()
                && name
                    .strip_prefix(content)
                    .is_some_and(|rest| rest.starts_with('\\'))
                && of(&name, &self.executable)
        });
        game || instance
    }

    /// The value the snapshot keeps for `name`, whatever its case.
    fn kept(&self, name: &str) -> Option<&Value> {
        self.values.get(name).or_else(|| {
            self.values
                .iter()
                .find(|(n, _)| plain(n) == plain(name))
                .map(|(_, v)| v)
        })
    }
}

/// Whether `value` is what Nucleus writes: [`NUCLEUS_DPI`] as REG_SZ.
fn nucleus_wrote(value: &Value) -> bool {
    const REG_SZ: u32 = 1;
    if value.kind != REG_SZ || !value.data.len().is_multiple_of(2) {
        return false;
    }
    let text: Vec<u16> = value
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let end = text.iter().rposition(|&c| c != 0).map_or(0, |at| at + 1);
    String::from_utf16(&text[..end]).is_ok_and(|text| text == NUCLEUS_DPI)
}

/// The values in `layers` now that Nucleus would set for `executable` at
/// `game`, with its instances below `content`.
pub fn snapshot(
    layers: &mut dyn Layers,
    executable: &str,
    game: &str,
    content: &Path,
) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        user: layers.user().to_string(),
        executable: executable.to_string(),
        game: Some(game.to_string()),
        content: Some(content.to_string_lossy().into_owned()),
        values: BTreeMap::new(),
    };
    snapshot.values = layers
        .values()?
        .into_iter()
        .filter(|(name, _)| snapshot.covers(name))
        .collect();
    Ok(snapshot)
}

/// Put `snapshot` back where Nucleus' DPI mode still is: such a value gets
/// its old data, or goes if it had none. A value the user changed or
/// removed since stays as they left it. Returns how many values it changed.
pub fn restore(layers: &mut dyn Layers, snapshot: &Snapshot) -> Result<usize> {
    let now = layers.values()?;
    let mut changed = 0;
    for (name, value) in now.iter().filter(|(name, _)| snapshot.covers(name)) {
        if !nucleus_wrote(value) {
            continue;
        }
        match snapshot.kept(name) {
            Some(old) if old == value => continue,
            Some(old) => layers.set(name, old)?,
            None => layers.delete(name)?,
        }
        changed += 1;
    }
    Ok(changed)
}

/// The file in Nucleus' `folder` that keeps `user`'s values until they are
/// restored.
fn journal(folder: &Path, user: &str) -> PathBuf {
    folder.join(format!("Rubylight-layers-{user}.json"))
}

/// Write `snapshot` to `journal` whole: a temporary file, then renamed over
/// it, so a crash never leaves half a file.
fn write_journal(journal: &Path, snapshot: &Snapshot) -> Result<()> {
    let text = serde_json::to_vec_pretty(snapshot)?;
    let temporary = journal.with_extension("json.tmp");
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, journal).with_context(|| format!("replacing {}", journal.display()))
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
    /// Before Nucleus Co-op at `folder` starts `executable` (the file name)
    /// of the game at `game`: restore what a session the host never ended
    /// left, then keep the values now.
    pub fn take(
        mut layers: Box<dyn Layers>,
        folder: &Path,
        executable: &str,
        game: &str,
    ) -> Result<Self> {
        let journal = journal(folder, layers.user());
        restore_left(layers.as_mut(), &journal)?;
        let snapshot = snapshot(layers.as_mut(), executable, game, &folder.join("content"))?;
        write_journal(&journal, &snapshot)?;
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
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading {}, which keeps the compatibility settings an earlier Nucleus Co-op session changed",
                    journal.display()
                )
            });
        }
    };
    // Each error names the file: deleting it lets sessions start again, with
    // the settings it kept left as Nucleus set them.
    let left = serde_json::from_slice::<Snapshot>(&text).map_err(|error| {
        anyhow::anyhow!(
            "{} can't be read ({error}): delete it, then start the stream again. The compatibility settings it kept stay as Nucleus Co-op set them",
            journal.display()
        )
    })?;
    let changed = restore(layers, &left).with_context(|| {
        format!(
            "the compatibility settings an earlier Nucleus Co-op session changed were not put back from {}; deleting it lets sessions start without putting them back",
            journal.display()
        )
    })?;
    tracing::info!(
        executable = left.executable,
        changed,
        "Nucleus Co-op: an earlier session's compatibility settings are back as they were"
    );
    std::fs::remove_file(journal).with_context(|| format!("removing {}", journal.display()))
}

/// Restore what a session the host never ended left for the user of
/// `layers` in the Nucleus Co-op at `folder`. Checked before a session
/// closes the console's window, and when the service sees the user sign in.
pub fn restore_left_in(layers: &mut dyn Layers, folder: &Path) -> Result<()> {
    let journal = journal(folder, layers.user());
    restore_left(layers, &journal)
}

/// For the service: restore what a session left for the user signed in at
/// the console, in the Nucleus Co-op `configured` names (`nucleus_path`) or
/// the host's own copy below `data`. Nothing without a signed-in user or
/// Nucleus Co-op. Call it only while no host runs a session for them, as
/// when one starts or the user just signed in.
pub fn restore_signed_in(data: &Path, configured: &str) -> Result<()> {
    let Some(folder) = super::install::installed(data, configured)? else {
        return Ok(());
    };
    if crate::process::user_sid().is_none() {
        return Ok(());
    }
    restore_left_in(&mut UserHive::signed_in()?, &folder)
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

    /// An instance's .exe below the content folder of the Nucleus at `folder`.
    fn instance(folder: &Path) -> String {
        folder
            .join(r"content\Rubylight-puzzle\Instance0\Puzzle.exe")
            .to_string_lossy()
            .into_owned()
    }

    fn take(map: &Map, folder: &Path) -> Result<Guard> {
        Guard::take(Box::new(map.clone()), folder, "puzzle.EXE", GAME)
    }

    #[test]
    fn the_users_settings_come_back_exactly_and_nucleus_values_go() {
        let map = Map::default();
        let other = r"C:\Tools\Other.exe";
        let before = BTreeMap::from([
            (GAME.to_string(), text("~ RUNASADMIN WIN7RTM")),
            (other.to_string(), text("~ DPIUNAWARE")),
        ]);
        *map.values.lock().unwrap() = before.clone();
        let folder = tempfile::tempdir().unwrap();
        let mut guard = take(&map, folder.path()).unwrap();
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        assert!(!journal.with_extension("json.tmp").exists());
        // Nucleus replaces the game's flags and sets its link's; the user
        // changes another program meanwhile.
        nucleus_runs(&map, &[GAME, &instance(folder.path())]);
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
    fn another_game_with_the_same_exe_name_keeps_what_the_user_set() {
        // Every RPG Maker game is Game.exe: the user's settings for another
        // one, made while the session ran, are theirs, even Nucleus' mode.
        let map = Map::default();
        let (old, new) = (r"E:\Old\Puzzle.exe", r"F:\New\Puzzle.exe");
        map.values
            .lock()
            .unwrap()
            .insert(old.into(), text("~ WIN7RTM"));
        let folder = tempfile::tempdir().unwrap();
        let guard = take(&map, folder.path()).unwrap();
        nucleus_runs(&map, &[GAME]);
        {
            let mut values = map.values.lock().unwrap();
            values.insert(old.into(), text("~ HIGHDPIAWARE"));
            values.insert(new.into(), text("~ RUNASADMIN"));
            // Outside the content folder, though it names it.
            values.insert(r"D:\content\Puzzle.exe".into(), text("~ HIGHDPIAWARE"));
        }
        drop(guard);
        let values = map.values.lock().unwrap();
        assert!(!values.contains_key(GAME));
        assert_eq!(values[old], text("~ HIGHDPIAWARE"));
        assert_eq!(values[new], text("~ RUNASADMIN"));
        assert_eq!(values[r"D:\content\Puzzle.exe"], text("~ HIGHDPIAWARE"));
    }

    #[test]
    fn a_value_the_user_changed_or_removed_since_stays_as_they_left_it() {
        let map = Map::default();
        let link = r"C:\Other\Puzzle.exe";
        map.values
            .lock()
            .unwrap()
            .insert(GAME.into(), text("~ WIN7RTM"));
        let folder = tempfile::tempdir().unwrap();
        let instance = instance(folder.path());
        map.values
            .lock()
            .unwrap()
            .insert(instance.clone(), text("~ WIN8RTM"));
        let guard = take(&map, folder.path()).unwrap();
        nucleus_runs(&map, &[GAME, &instance]);
        {
            let mut values = map.values.lock().unwrap();
            values.insert(GAME.into(), text("~ RUNASADMIN"));
            values.remove(&instance);
            values.insert(link.into(), text("~ HIGHDPIAWARE"));
        }
        drop(guard);
        let values = map.values.lock().unwrap();
        assert_eq!(values[GAME], text("~ RUNASADMIN"));
        assert!(!values.contains_key(&instance));
        assert_eq!(values[link], text("~ HIGHDPIAWARE"));
    }

    #[test]
    fn only_nucleus_dpi_mode_counts_as_nucleus() {
        assert!(nucleus_wrote(&text("~ HIGHDPIAWARE")));
        // Without the terminating zero, as some programs write it.
        let mut bare = text("~ HIGHDPIAWARE");
        bare.data.truncate(bare.data.len() - 2);
        assert!(nucleus_wrote(&bare));
        assert!(!nucleus_wrote(&text("~ HIGHDPIAWARE RUNASADMIN")));
        assert!(!nucleus_wrote(&text("~ highdpiaware")));
        assert!(!nucleus_wrote(&Value {
            kind: 2,
            data: text("~ HIGHDPIAWARE").data
        }));
        assert!(!nucleus_wrote(&Value {
            kind: 1,
            data: vec![1, 2, 3]
        }));
    }

    #[test]
    fn a_file_an_earlier_build_wrote_restores_nucleus_values_by_file_name() {
        let map = Map::default();
        let folder = tempfile::tempdir().unwrap();
        let journal = journal(folder.path(), "S-1-5-21-7");
        std::fs::write(
            &journal,
            r#"{"user":"S-1-5-21-7","executable":"Puzzle.exe","values":{"E:\\Old\\Puzzle.exe":{"kind":1,"data":[126,0,32,0,87,0,73,0,78,0,56,0,82,0,84,0,77,0,0,0]}}}"#,
        )
        .unwrap();
        {
            let mut values = map.values.lock().unwrap();
            values.insert(r"E:\Old\Puzzle.exe".into(), text("~ HIGHDPIAWARE"));
            values.insert(r"E:\Mine\Puzzle.exe".into(), text("~ RUNASADMIN"));
        }
        restore_left_in(&mut map.clone(), folder.path()).unwrap();
        let values = map.values.lock().unwrap();
        assert_eq!(values[r"E:\Old\Puzzle.exe"], text("~ WIN8RTM"));
        assert_eq!(values[r"E:\Mine\Puzzle.exe"], text("~ RUNASADMIN"));
        assert!(!journal.exists());
    }

    #[test]
    fn an_unreadable_file_names_itself_to_delete_and_then_sessions_start() {
        let map = Map::default();
        let folder = tempfile::tempdir().unwrap();
        let journal = journal(folder.path(), "S-1-5-21-7");
        std::fs::write(&journal, "{\"user\":").unwrap();
        let error = format!("{:#}", take(&map, folder.path()).err().unwrap());
        assert!(error.contains(&journal.display().to_string()), "{error}");
        assert!(error.contains("delete it"), "{error}");
        assert!(journal.is_file());
        std::fs::remove_file(&journal).unwrap();
        drop(take(&map, folder.path()).unwrap());
    }

    #[test]
    fn a_game_without_settings_has_none_again_and_a_dropped_guard_restores() {
        let map = Map::default();
        let folder = tempfile::tempdir().unwrap();
        let guard = take(&map, folder.path()).unwrap();
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
        let guard = take(&map, folder.path()).unwrap();
        nucleus_runs(&map, &[GAME]);
        // The host stops before the session ends.
        std::mem::forget(guard);
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        // The next session, of another game, restores first, then keeps its own.
        let next = Guard::take(
            Box::new(map.clone()),
            folder.path(),
            "Other.exe",
            r"C:\Other\Other.exe",
        )
        .unwrap();
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
        let kept: Snapshot = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        assert_eq!(kept.executable, "Other.exe");
        assert_eq!(kept.game.as_deref(), Some(r"C:\Other\Other.exe"));
        drop(next);
        assert!(!journal.exists());
    }

    #[test]
    fn values_left_by_a_host_that_stopped_come_back_when_the_user_signs_in() {
        let map = Map::default();
        map.values
            .lock()
            .unwrap()
            .insert(GAME.into(), text("~ WIN8RTM"));
        let folder = tempfile::tempdir().unwrap();
        std::mem::forget(take(&map, folder.path()).unwrap());
        nucleus_runs(&map, &[GAME]);
        // What the service runs as the user signs in, before any session.
        restore_left_in(&mut map.clone(), folder.path()).unwrap();
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
        assert!(!journal(folder.path(), "S-1-5-21-7").exists());
        // Nothing left: nothing to do.
        restore_left_in(&mut map.clone(), folder.path()).unwrap();
    }

    #[test]
    fn values_that_cannot_be_restored_are_kept_and_block_the_next_session() {
        let map = Map::default();
        map.values
            .lock()
            .unwrap()
            .insert(GAME.into(), text("~ WIN8RTM"));
        let folder = tempfile::tempdir().unwrap();
        let guard = take(&map, folder.path()).unwrap();
        nucleus_runs(&map, &[GAME]);
        *map.broken.lock().unwrap() = true;
        drop(guard);
        let journal = journal(folder.path(), "S-1-5-21-7");
        assert!(journal.is_file());
        // No session may snapshot Nucleus' value as the user's.
        let error = take(&map, folder.path()).err().unwrap();
        let error = format!("{error:#}");
        assert!(error.contains("were not put back from"), "{error}");
        assert!(error.contains("deleting it lets sessions start"), "{error}");
        assert!(journal.is_file());
        *map.broken.lock().unwrap() = false;
        drop(take(&map, folder.path()).unwrap());
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
