//! How two streams' virtual displays were arranged, kept for the next time the same two stream
//! together: where the second sits from the first, and which of them is the main display.
//! Without it every stream puts them in the order they arrived, the first main and the second
//! to its right, and the two screens of one device (Dual screen mode) arrive together in either
//! order. Kept by display, so it holds for any two streams' virtual displays, a device's two
//! screens being the case it is for; physical displays are never moved from memory.
use anyhow::Result;
use butterpollo_core::topology::{Node, Position};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
/// The pairs of displays kept; the oldest goes first.
const KEPT: usize = 16;
static PATH: OnceLock<PathBuf> = OnceLock::new();
static SAVED: Mutex<Vec<Saved>> = Mutex::new(Vec::new());
/// Two displays by their ids, in order (`a` < `b`, so the order they arrived in does not matter),
/// with `b`'s position from `a`'s and the main one of the two, if either was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Saved {
    a: String,
    b: String,
    dx: i32,
    dy: i32,
    main: Option<bool>,
}
/// Read the pairs kept in `directory`. A missing or unreadable file keeps none.
pub fn initialize(directory: &Path) {
    let path = directory.join("stream-display-layouts.json");
    let saved = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Vec<Saved>>(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "the kept arrangements of stream displays are unreadable; starting without them");
            Vec::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the kept arrangements of stream displays could not be read; starting without them");
            Vec::new()
        }
    };
    *SAVED.lock().unwrap() = saved;
    let _ = PATH.set(path);
}
/// Whether the displays of the streams now, `targets`, have an arrangement to keep: two, both
/// virtual (none of `physical`).
pub(super) fn kept_for(targets: &[String], physical: &[String]) -> bool {
    ordered(targets).is_some() && !targets.iter().any(|t| physical.contains(t))
}
fn node<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.active && n.device_id == id)
}
fn ordered(targets: &[String]) -> Option<(&str, &str)> {
    match targets {
        [x, y] if x != y => Some(if x < y { (x, y) } else { (y, x) }),
        _ => None,
    }
}
/// The arrangement of the two displays in `nodes`, if both are on.
pub(super) fn capture(nodes: &[Node], targets: &[String]) -> Option<Saved> {
    let (a, b) = ordered(targets)?;
    let (na, nb) = (node(nodes, a)?, node(nodes, b)?);
    Some(Saved {
        a: a.to_owned(),
        b: b.to_owned(),
        dx: nb.desired_position.x.checked_sub(na.desired_position.x)?,
        dy: nb.desired_position.y.checked_sub(na.desired_position.y)?,
        main: if na.primary {
            Some(false)
        } else if nb.primary {
            Some(true)
        } else {
            None
        },
    })
}
fn overlap(x: &Node, y: &Node) -> bool {
    let extent = |n: &Node| {
        let (w, h) = (i64::from(n.mode.width), i64::from(n.mode.height));
        let (px, py) = (
            i64::from(n.desired_position.x),
            i64::from(n.desired_position.y),
        );
        (px, py, px + w, py + h)
    };
    let (a, b) = (extent(x), extent(y));
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}
/// The layout `desired` with the kept arrangement of the two displays in `targets`, if one is
/// kept and fits: both on, not over another display, with the main display at the origin as
/// Windows needs it. `streams_main`: the layout makes a stream's display the main one, which
/// the kept choice then decides; otherwise the main display stays what it is.
pub(super) fn arranged(
    desired: &[Node],
    targets: &[String],
    streams_main: bool,
    saved: &[Saved],
) -> Option<Vec<Node>> {
    let (a, b) = ordered(targets)?;
    let kept = saved.iter().find(|s| s.a == a && s.b == b)?;
    let anchor = node(desired, a)?.desired_position;
    node(desired, b)?;
    let mut next = desired.to_vec();
    next.iter_mut().find(|n| n.device_id == b)?.desired_position = Position {
        x: anchor.x.checked_add(kept.dx)?,
        y: anchor.y.checked_add(kept.dy)?,
    };
    let main = match (streams_main, kept.main) {
        (true, Some(second)) => Some((if second { b } else { a }).to_owned()),
        _ => next
            .iter()
            .find(|n| n.active && n.primary)
            .map(|n| n.device_id.clone()),
    };
    if let Some(main) = main {
        let origin = node(&next, &main)?.desired_position;
        for n in &mut next {
            n.primary = n.device_id == main;
            n.desired_position = Position {
                x: n.desired_position.x.checked_sub(origin.x)?,
                y: n.desired_position.y.checked_sub(origin.y)?,
            };
        }
    }
    let active: Vec<_> = next.iter().filter(|n| n.active).collect();
    let crowded = active
        .iter()
        .enumerate()
        .any(|(i, x)| active[i + 1..].iter().any(|y| overlap(x, y)));
    (!crowded).then_some(next)
}
/// `desired` with the arrangement kept for the two displays in `targets`, as it is when none is.
pub(super) fn apply(desired: Vec<Node>, targets: &[String], streams_main: bool) -> Vec<Node> {
    let saved = SAVED.lock().unwrap();
    match arranged(&desired, targets, streams_main, &saved) {
        Some(next) => {
            tracing::info!(displays = ?targets, "laying out two streams' displays as the user left them");
            next
        }
        None => desired,
    }
}
/// Keep the arrangement of the two displays in `targets` as `nodes` has them now.
pub(super) fn remember(nodes: &[Node], targets: &[String]) {
    let Some(now) = capture(nodes, targets) else {
        return;
    };
    let mut saved = SAVED.lock().unwrap();
    if saved.last() == Some(&now) {
        return;
    }
    saved.retain(|s| !(s.a == now.a && s.b == now.b));
    saved.push(now);
    if saved.len() > KEPT {
        saved.remove(0);
    }
    if let Some(path) = PATH.get()
        && let Err(error) = write(path, &saved)
    {
        tracing::warn!(error = %format!("{error:#}"), "keeping the arrangement of two streams' displays failed");
    }
}
fn write(path: &Path, saved: &[Saved]) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(saved)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::topology::{Kind, Mode};
    fn node(id: &str, x: i32, y: i32, primary: bool) -> Node {
        Node {
            id: id.into(),
            label: id.into(),
            kind: Kind::Client,
            active: true,
            primary,
            desired_position: Position { x, y },
            mode: Mode {
                width: 1000,
                height: 800,
                refresh_hz: 60.0,
            },
            device_id: id.into(),
        }
    }
    fn targets(a: &str, b: &str) -> Vec<String> {
        vec![a.into(), b.into()]
    }
    #[test]
    fn the_arrangement_is_the_same_whichever_screen_arrived_first() {
        let left_main = [node("a", 0, 0, true), node("b", 1000, 0, false)];
        let one = capture(&left_main, &targets("a", "b")).unwrap();
        let other = capture(&left_main, &targets("b", "a")).unwrap();
        assert_eq!(one, other);
        assert_eq!((one.dx, one.dy, one.main), (1000, 0, Some(false)));
    }
    #[test]
    fn a_kept_main_display_and_side_come_back_whichever_arrived_first() {
        // The user made `b` the main display, with `a` to its left.
        let kept = capture(
            &[node("a", -1000, 0, false), node("b", 0, 0, true)],
            &targets("a", "b"),
        )
        .unwrap();
        // The host's default: the first to arrive is main, the second to its right.
        let default = vec![node("b", 0, 0, true), node("a", 1000, 0, false)];
        let next = arranged(&default, &targets("b", "a"), true, &[kept]).unwrap();
        let at = |id: &str| next.iter().find(|n| n.device_id == id).unwrap();
        assert_eq!(at("b").desired_position, Position { x: 0, y: 0 });
        assert!(at("b").primary && !at("a").primary);
        assert_eq!(at("a").desired_position, Position { x: -1000, y: 0 });
    }
    #[test]
    fn an_arrangement_that_would_overlap_another_display_is_not_kept() {
        let kept = capture(
            &[node("a", 1000, 0, false), node("b", 0, 0, true)],
            &targets("a", "b"),
        )
        .unwrap();
        // `b` would land 1000 to the left of `a`, on the desk's own display.
        let desired = vec![
            node("desk", 0, 0, true),
            node("a", 1000, 0, false),
            node("b", 2000, 0, false),
        ];
        assert!(arranged(&desired, &targets("a", "b"), false, &[kept]).is_none());
    }
    #[test]
    fn the_main_display_stays_when_the_layout_does_not_make_a_stream_main() {
        let kept = capture(
            &[node("a", 0, 0, false), node("b", 1000, 0, true)],
            &targets("a", "b"),
        )
        .unwrap();
        let desired = vec![
            node("desk", -1000, 0, false),
            node("a", 3000, 0, false),
            node("b", 4000, 0, false),
            node("main", 0, 0, true),
        ];
        let next = arranged(&desired, &targets("a", "b"), false, &[kept]).unwrap();
        let main = next.iter().find(|n| n.device_id == "main").unwrap();
        assert!(main.primary && main.desired_position == Position { x: 0, y: 0 });
    }
    #[test]
    fn only_two_virtual_displays_have_an_arrangement_kept() {
        let physical = ["monitor".to_owned()];
        assert!(kept_for(&targets("a", "b"), &physical));
        assert!(!kept_for(&targets("a", "monitor"), &physical));
        assert!(!kept_for(&targets("a", "a"), &physical));
        assert!(!kept_for(&["a".to_owned()], &physical));
        assert!(!kept_for(&["a".into(), "b".into(), "c".into()], &physical));
    }
    #[test]
    fn nothing_changes_without_a_kept_arrangement_or_with_one_display() {
        let desired = vec![node("a", 0, 0, true), node("b", 1000, 0, false)];
        assert!(arranged(&desired, &targets("a", "b"), true, &[]).is_none());
        assert!(capture(&desired, &["a".to_owned()]).is_none());
    }
}
