//! Quick Suspend: pausing a running game's process tree and resuming it later.
//!
//! The OS-free half lives here: the write-ahead journal that names every
//! process the host is about to suspend ([`journal`]), and the recovery that
//! any later host, or the watchdog the host leaves beside it, runs over that
//! journal so no game stays frozen after a crash ([`recover`]). The Windows
//! calls are in `butterpollo_win_io::process::suspend`, the host's bookkeeping
//! in `butterpollo_stream::quick_suspend`.
//!
//! The journal is a file of its own (`quick-suspend.json` in the profile), not
//! a section of the display crash-restore journal: the two are written by
//! different code at different times, and each can be read and cleared alone.
pub mod journal;

pub use journal::{FILE, Journal, Record};

/// What resuming one recorded process found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resumed {
    /// It ran under the recorded identity and was resumed.
    Done,
    /// It exited, or its id now belongs to another process: nothing was done.
    Gone,
}

/// Resumes one process, if it still runs under the identity (`pid`, creation
/// time in 100 ns since 1601) the journal recorded.
pub trait Thaw {
    fn resume(&mut self, pid: u32, started: u64) -> Result<Resumed, String>;
}

/// What a recovery did, for its log line and the console's notice.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The program names that were resumed.
    pub resumed: Vec<String>,
    /// How many recorded processes had already exited.
    pub gone: usize,
    /// The processes that could not be resumed, with why.
    pub failed: Vec<(Record, String)>,
}

impl Recovery {
    /// Whether every recorded process now runs or has exited.
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Resume every process `journal` records, parents before their children (the
/// reverse of the order they were suspended in). A process that exited or whose
/// id was reused is left alone. A failure does not stop the rest.
pub fn recover(journal: &Journal, thaw: &mut impl Thaw) -> Recovery {
    let mut recovery = Recovery::default();
    for record in journal.processes.iter().rev() {
        match thaw.resume(record.pid, record.started) {
            Ok(Resumed::Done) => recovery.resumed.push(record.name.clone()),
            Ok(Resumed::Gone) => recovery.gone += 1,
            Err(error) => recovery.failed.push((record.clone(), error)),
        }
    }
    recovery
}

/// Whether a host started from `profile` must recover the journal it finds:
/// always, unless the journal belongs to another host that still runs (`alive`
/// tells whether a process runs under an identity). A host never recovers a
/// journal it wrote itself (`me`), which it resumes on its own.
pub fn stale(journal: &Journal, me: (u32, u64), alive: impl Fn(u32, u64) -> bool) -> bool {
    let owner = (journal.host_pid, journal.host_started);
    owner != me && !(owner.1 != 0 && alive(owner.0, owner.1))
}

#[cfg(test)]
mod tests;
