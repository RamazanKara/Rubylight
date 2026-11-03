//! Process selection for game-only audio. Never select a desktop/launcher by
//! name or foreground window; identities must come from the owned launch job
//! or the launcher's existing game tracker, including creation times.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity {
    pub pid: u32,
    pub started: u64,
}

/// A process loopback includes descendants. Select the highest verified game
/// process so children are not captured twice. Refuse ambiguous unrelated
/// trees instead of widening capture to their shared, unowned launcher.
pub fn root(owned: &BTreeMap<u32, u64>, processes: &[crate::steam::Process]) -> Result<Identity> {
    let candidates: BTreeMap<_, _> = processes
        .iter()
        .filter(|p| p.pid != 0 && p.started != 0 && owned.get(&p.pid) == Some(&p.started))
        .map(|p| (p.pid, p))
        .collect();
    let roots: Vec<_> = candidates
        .values()
        .filter(|p| {
            let mut child = **p;
            for _ in 0..processes.len() {
                let Some(parent) = processes.iter().find(|q| {
                    q.pid == child.parent
                        && q.started > 0
                        && q.started <= child.started
                        && q.pid != child.pid
                }) else {
                    break;
                };
                if candidates.contains_key(&parent.pid) {
                    return false;
                }
                child = parent;
            }
            true
        })
        .collect();
    ensure!(
        !roots.is_empty(),
        "the launched game has no verified running process yet"
    );
    ensure!(
        roots.len() == 1,
        "the launch has several independent process trees; use a direct game executable entry for game-only audio"
    );
    Ok(Identity {
        pid: roots[0].pid,
        started: roots[0].started,
    })
}

/// `root` and its running descendants: the processes a process loopback of
/// `root` captures. A child started before its parent is another process
/// that reused a dead parent's ID, not a descendant.
pub fn tree(root: Identity, processes: &[crate::steam::Process]) -> Vec<Identity> {
    if !processes
        .iter()
        .any(|p| p.pid == root.pid && p.started == root.started)
    {
        return vec![];
    }
    let mut found = vec![root];
    let mut at = 0;
    while let Some(parent) = found.get(at).copied() {
        at += 1;
        for child in processes.iter().filter(|p| {
            p.parent == parent.pid
                && p.pid != parent.pid
                && p.started != 0
                && p.started >= parent.started
        }) {
            let child = Identity {
                pid: child.pid,
                started: child.started,
            };
            if !found.contains(&child) {
                found.push(child);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_tree_is_the_root_and_its_descendants_only() {
        let root = Identity {
            pid: 10,
            started: 100,
        };
        let all = [
            process(10, 1, 100),
            process(11, 10, 110),
            process(12, 11, 120),
            // An older process whose dead parent's ID the root reused.
            process(13, 10, 50),
            process(14, 2, 130),
        ];
        let pids: Vec<u32> = tree(root, &all).iter().map(|i| i.pid).collect();
        assert_eq!(pids, [10, 11, 12]);
        assert!(
            tree(
                Identity {
                    pid: 10,
                    started: 99
                },
                &all
            )
            .is_empty()
        );
    }
    fn process(pid: u32, parent: u32, started: u64) -> crate::steam::Process {
        crate::steam::Process {
            pid,
            parent,
            started,
            name: String::new(),
        }
    }
    #[test]
    fn children_are_included_once_and_outlive_the_launcher() {
        let owned = BTreeMap::from([(10, 100), (11, 110), (12, 120)]);
        let processes = vec![
            process(10, 2, 100),
            process(11, 10, 110),
            process(12, 11, 120),
            process(99, 2, 99),
        ];
        assert_eq!(root(&owned, &processes).unwrap().pid, 10);
        assert_eq!(root(&owned, &processes[1..]).unwrap().pid, 11);
    }
    #[test]
    fn reused_pids_or_ambiguous_trees_never_widen_to_system_audio() {
        let owned = BTreeMap::from([(10, 100), (11, 110)]);
        assert!(root(&owned, &[process(10, 2, 999)]).is_err());
        assert!(root(&owned, &[process(10, 2, 100), process(11, 2, 110)]).is_err());
        assert!(root(&BTreeMap::new(), &[process(10, 2, 100)]).is_err());
    }
    #[test]
    fn a_reused_parent_is_not_an_ancestor() {
        let owned = BTreeMap::from([(10, 100), (11, 110)]);
        assert!(
            root(
                &owned,
                &[
                    process(10, 2, 100),
                    process(11, 20, 110),
                    process(20, 10, 500)
                ]
            )
            .is_err()
        );
    }
}
