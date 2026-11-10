//! The PC's process list in one system call.
//!
//! A Toolhelp snapshot names every process and its parent, but not when it started: that took
//! opening each process, a few hundred handles for every list, and a Steam game is looked for in
//! the list once a second for as long as it runs. The kernel's own list carries the start times,
//! so a process is opened once, the first time it is listed, and not again while it lives.
//!
//! A start time is still reported only for a process this one may open, as before: callers tell
//! the processes of other sessions and of the system apart by the zero an unreadable one gets.

use butterpollo_core::steam::Process;
use std::{collections::HashSet, ffi::c_void, mem::size_of};

/// `SystemProcessInformation`.
const PROCESS_INFORMATION: u32 = 5;
/// `STATUS_INFO_LENGTH_MISMATCH`: the buffer was too small, and the length the list needs came
/// back.
const LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
/// The first buffer: the list of a PC with a few hundred processes fits.
const FIRST_BYTES: usize = 512 * 1024;
/// Room for the processes and threads that start between the call that says how much the list
/// needs and the one that reads it.
const SPARE_BYTES: usize = 64 * 1024;
/// No list is this long; a length beyond it is not believed.
const MOST_BYTES: usize = 64 * 1024 * 1024;

/// The head of a `SYSTEM_PROCESS_INFORMATION` entry on 64-bit Windows. The fields after the
/// parent and the thread entries that follow them are not read: `next` skips them.
#[repr(C)]
struct Entry {
    /// Bytes from this entry to the next one, 0 in the last.
    next: u32,
    threads: u32,
    /// Private working set, hard faults, thread high-water mark and cycle time.
    unused: [u64; 3],
    /// Creation time, 100 ns since 1601.
    created: i64,
    user_time: i64,
    kernel_time: i64,
    /// The program name's length in bytes.
    name_bytes: u16,
    name_capacity: u16,
    /// The name's UTF-16 units, inside the same buffer.
    name: *const u16,
    priority: i32,
    pid: usize,
    parent: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// A process as the kernel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    pid: u32,
    parent: u32,
    /// Creation time, 100 ns since 1601.
    created: u64,
    name: String,
}

/// Fill `buffer` with the kernel's process list; the bytes it wrote. `None` when Windows refuses
/// the list.
fn read(buffer: &mut Vec<u64>) -> Option<usize> {
    if buffer.is_empty() {
        buffer.resize(FIRST_BYTES / size_of::<u64>(), 0);
    }
    for _ in 0..4 {
        let size = buffer.len() * size_of::<u64>();
        let mut returned = 0u32;
        // SAFETY: `buffer` is writable for `size` bytes, which fits a u32 (it never grows past
        // `MOST_BYTES`), and `returned` is a local the call may write.
        let status = unsafe {
            NtQuerySystemInformation(
                PROCESS_INFORMATION,
                buffer.as_mut_ptr().cast(),
                size as u32,
                &mut returned,
            )
        };
        if status >= 0 {
            let written = returned as usize;
            // The entries chain to their own end, so a length Windows left out is the buffer's.
            return Some(if written == 0 {
                size
            } else {
                written.min(size)
            });
        }
        if status != LENGTH_MISMATCH {
            return None;
        }
        let wanted = (returned as usize).max(size) + SPARE_BYTES;
        if wanted > MOST_BYTES {
            return None;
        }
        buffer.resize(wanted.div_ceil(size_of::<u64>()), 0);
    }
    None
}

/// The processes in the first `length` bytes of a list [`read`] filled. An entry or a name that
/// would lie outside those bytes ends the list or is left empty: nothing outside is read.
fn parse(buffer: &[u64], length: usize) -> Vec<Listed> {
    let length = length.min(std::mem::size_of_val(buffer));
    let base = buffer.as_ptr().cast::<u8>();
    let mut list = vec![];
    let mut at = 0usize;
    while at.is_multiple_of(align_of::<Entry>())
        && at
            .checked_add(size_of::<Entry>())
            .is_some_and(|end| end <= length)
    {
        // SAFETY: the whole entry lies inside `buffer` (checked above) at an offset aligned for
        // it, in memory aligned for u64. Every field is an integer or a pointer that is not
        // followed here, so any bytes are a valid `Entry`.
        let entry = unsafe { base.add(at).cast::<Entry>().read() };
        // The idle process is no program.
        if entry.pid != 0 {
            list.push(Listed {
                pid: entry.pid as u32,
                parent: entry.parent as u32,
                created: entry.created.max(0) as u64,
                name: name(&entry, buffer, length),
            });
        }
        if entry.next == 0 {
            break;
        }
        let Some(next) = at.checked_add(entry.next as usize) else {
            break;
        };
        at = next;
    }
    list
}

/// The entry's program name, empty when it does not lie inside the list.
fn name(entry: &Entry, buffer: &[u64], length: usize) -> String {
    let start = buffer.as_ptr() as usize;
    let at = entry.name as usize;
    let bytes = usize::from(entry.name_bytes);
    let inside = at >= start
        && at.is_multiple_of(align_of::<u16>())
        && at - start <= length
        && bytes <= length - (at - start);
    if !inside || bytes == 0 {
        return String::new();
    }
    // SAFETY: the name's bytes lie inside `buffer` (checked above), which outlives this call, at
    // an address aligned for u16.
    let units = unsafe { std::slice::from_raw_parts(entry.name, bytes / size_of::<u16>()) };
    String::from_utf16_lossy(units)
}

/// The process list, and which of its processes are known to be readable.
#[derive(Default)]
pub(super) struct Listing {
    /// Kept between lists: a list is a few hundred kilobytes.
    buffer: Vec<u64>,
    /// The processes, by ID and creation time, that were opened and gave that creation time.
    readable: HashSet<(u32, u64)>,
}

impl Listing {
    /// Every running process with its parent, start time and program name. `started` opens a
    /// process for its creation time (0 when it may not be opened); it is asked only for a
    /// process not yet known to be readable. `None` when Windows gives no list.
    pub(super) fn processes(&mut self, started: impl Fn(u32) -> u64) -> Option<Vec<Process>> {
        let length = read(&mut self.buffer)?;
        let listed = parse(&self.buffer, length);
        if listed.is_empty() {
            return None;
        }
        Some(self.resolve(listed, started))
    }

    fn resolve(&mut self, listed: Vec<Listed>, started: impl Fn(u32) -> u64) -> Vec<Process> {
        let mut live = HashSet::with_capacity(listed.len());
        let list = listed
            .into_iter()
            .map(|listed| {
                let key = (listed.pid, listed.created);
                let started = if listed.created != 0 && self.readable.contains(&key) {
                    listed.created
                } else {
                    started(listed.pid)
                };
                // Another process may have taken the ID between the list and the open: only a
                // time that is the listed one is remembered for the listed process.
                if started != 0 && started == listed.created {
                    live.insert(key);
                }
                Process {
                    pid: listed.pid,
                    parent: listed.parent,
                    started,
                    name: listed.name,
                }
            })
            .collect();
        // Processes that are gone are forgotten with the list they are missing from.
        self.readable = live;
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn listed(pid: u32, created: u64) -> Listed {
        Listed {
            pid,
            parent: 4,
            created,
            name: format!("{pid}.exe"),
        }
    }

    #[test]
    fn a_process_is_opened_once_and_again_only_when_its_id_was_reused() {
        let mut listing = Listing::default();
        let opened = RefCell::new(vec![]);
        // 30 may not be opened; the others give the time they were listed with.
        let times = RefCell::new(std::collections::HashMap::from([(10, 100), (20, 200)]));
        let started = |pid: u32| {
            opened.borrow_mut().push(pid);
            times.borrow().get(&pid).copied().unwrap_or(0)
        };
        let first = listing.resolve(
            vec![listed(10, 100), listed(20, 200), listed(30, 300)],
            started,
        );
        assert_eq!(
            first.iter().map(|p| (p.pid, p.started)).collect::<Vec<_>>(),
            [(10, 100), (20, 200), (30, 0)]
        );
        assert_eq!(std::mem::take(&mut *opened.borrow_mut()), [10, 20, 30]);

        // The same processes again: only the unreadable one is tried.
        let again = listing.resolve(
            vec![listed(10, 100), listed(20, 200), listed(30, 300)],
            started,
        );
        assert_eq!(
            again.iter().map(|p| (p.pid, p.started)).collect::<Vec<_>>(),
            [(10, 100), (20, 200), (30, 0)]
        );
        assert_eq!(std::mem::take(&mut *opened.borrow_mut()), [30]);

        // 20 exited and its ID went to a new process: that one is opened.
        times.borrow_mut().insert(20, 250);
        let reused = listing.resolve(vec![listed(10, 100), listed(20, 250)], started);
        assert_eq!(
            reused
                .iter()
                .map(|p| (p.pid, p.started))
                .collect::<Vec<_>>(),
            [(10, 100), (20, 250)]
        );
        assert_eq!(std::mem::take(&mut *opened.borrow_mut()), [20]);

        // 10 was missing from a list: it is not taken for readable from an older one.
        listing.resolve(vec![listed(20, 250)], started);
        opened.borrow_mut().clear();
        listing.resolve(vec![listed(10, 100), listed(20, 250)], started);
        assert_eq!(std::mem::take(&mut *opened.borrow_mut()), [10]);
    }

    #[test]
    fn a_time_read_from_another_process_under_the_same_id_is_not_remembered() {
        let mut listing = Listing::default();
        let opened = RefCell::new(0);
        // The ID was reused between the list and the open: the open gives the newcomer's time.
        let started = |_| {
            *opened.borrow_mut() += 1;
            999
        };
        let list = listing.resolve(vec![listed(10, 100)], started);
        assert_eq!(list[0].started, 999);
        listing.resolve(vec![listed(10, 100)], started);
        assert_eq!(*opened.borrow(), 2);
    }

    /// Lay out entries as Windows does: each followed by its name, the last with `next` 0.
    fn buffer(entries: &[(usize, usize, i64, &str)]) -> (Vec<u64>, usize) {
        let entry_words = size_of::<Entry>().div_ceil(8);
        let mut words = vec![0u64; entries.len() * (entry_words + 8)];
        let base = words.as_mut_ptr();
        let mut at = 0usize;
        for (index, (pid, parent, created, name)) in entries.iter().enumerate() {
            let units: Vec<u16> = name.encode_utf16().collect();
            let name_at = at + entry_words;
            let next = entry_words + 8;
            // SAFETY: the entry and its name lie inside `words`, at word offsets.
            unsafe {
                let text = base.add(name_at).cast::<u16>();
                std::ptr::copy_nonoverlapping(units.as_ptr(), text, units.len());
                base.add(at).cast::<Entry>().write(Entry {
                    next: if index + 1 == entries.len() {
                        0
                    } else {
                        (next * 8) as u32
                    },
                    threads: 1,
                    unused: [0; 3],
                    created: *created,
                    user_time: 0,
                    kernel_time: 0,
                    name_bytes: (units.len() * 2) as u16,
                    name_capacity: (units.len() * 2) as u16,
                    name: text,
                    priority: 8,
                    pid: *pid,
                    parent: *parent,
                });
            }
            at += next;
        }
        let length = words.len() * 8;
        (words, length)
    }

    #[test]
    fn the_kernel_list_is_read_entry_by_entry() {
        let (words, length) = buffer(&[
            (0, 0, 0, ""),
            (4, 0, 10, "System"),
            (1234, 4, 20, "spiel.exe"),
        ]);
        assert_eq!(
            parse(&words, length),
            [
                Listed {
                    pid: 4,
                    parent: 0,
                    created: 10,
                    name: "System".into()
                },
                Listed {
                    pid: 1234,
                    parent: 4,
                    created: 20,
                    name: "spiel.exe".into()
                },
            ]
        );
    }

    #[test]
    fn nothing_outside_the_list_is_read() {
        let (mut words, length) = buffer(&[(4, 0, 10, "System"), (1234, 4, 20, "spiel.exe")]);
        // Cut inside the second entry: it is not read.
        let cut = (size_of::<Entry>().div_ceil(8) + 8) * 8 + 16;
        assert_eq!(parse(&words, cut).len(), 1);
        // A list shorter than one entry holds nothing.
        assert!(parse(&words, size_of::<Entry>() - 1).is_empty());
        assert!(parse(&[], 4096).is_empty());
        // A name that points outside the list is left empty, and a `next` that does ends it.
        let outside = [0u16; 4];
        // SAFETY: the first entry lies inside `words`.
        unsafe {
            let first = words.as_mut_ptr().cast::<Entry>();
            (*first).name = outside.as_ptr();
            (*first).next = u32::MAX - 7;
        }
        let list = parse(&words, length);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "");
        // An unaligned `next` ends the list as well.
        // SAFETY: as above.
        unsafe { (*words.as_mut_ptr().cast::<Entry>()).next = 3 };
        assert_eq!(parse(&words, length).len(), 1);
    }

    #[test]
    fn the_kernel_list_agrees_with_the_snapshot() {
        let me = std::process::id();
        let mut listing = Listing::default();
        let kernel = listing
            .processes(crate::process::creation_time)
            .expect("the kernel lists processes");
        let snapshot = crate::process::snapshot_processes().unwrap();
        let mine = kernel.iter().find(|p| p.pid == me).unwrap();
        let theirs = snapshot.iter().find(|p| p.pid == me).unwrap();
        assert_eq!(
            (mine.parent, mine.started, &mine.name),
            (theirs.parent, theirs.started, &theirs.name)
        );
        assert!(mine.started > 0);
        // Processes come and go between the two lists, but not most of them.
        let both = kernel
            .iter()
            .filter(|k| {
                snapshot.iter().any(|s| {
                    (s.pid, s.parent, s.started, &s.name) == (k.pid, k.parent, k.started, &k.name)
                })
            })
            .count();
        assert!(
            both * 10 >= kernel.len().max(snapshot.len()) * 9,
            "{both} of {} and {} agree",
            kernel.len(),
            snapshot.len()
        );
        // The second list opens this process no more.
        let opened = RefCell::new(vec![]);
        let again = listing
            .processes(|pid| {
                opened.borrow_mut().push(pid);
                crate::process::creation_time(pid)
            })
            .unwrap();
        assert!(!opened.borrow().contains(&me));
        assert_eq!(
            again.iter().find(|p| p.pid == me).unwrap().started,
            mine.started
        );
    }
}
