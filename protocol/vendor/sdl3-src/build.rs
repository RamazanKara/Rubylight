//! Copies the SDL submodule into OUT_DIR (without .git, test and examples, as the crates.io
//! crate ships it) and applies every patch in patches/ there; the submodule stays pristine. The
//! patches are unified diffs applied hunk by hunk at their exact context, in order: a hunk that
//! does not match fails the build, so a patch never applies partly or silently.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("SDL");
    let patches = manifest.join("patches");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", patches.display());
    println!("cargo:rerun-if-changed={}", source.display());
    if !source.join("CMakeLists.txt").is_file() {
        panic!(
            "{} is empty: the SDL source is a git submodule. Run `git submodule update --init --depth 1 {}` in the repository.",
            source.display(),
            "client/protocol/vendor/sdl3-src/SDL (protocol/vendor/sdl3-src/SDL in the nightjar repository)"
        );
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("SDL");
    if out.exists() {
        fs::remove_dir_all(&out).unwrap_or_else(|e| panic!("cannot clear {}: {e}", out.display()));
    }
    copy_tree(&source, &out, true);
    let mut names: Vec<PathBuf> = fs::read_dir(&patches)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", patches.display()))
        .map(|entry| entry.expect("patch directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "patch"))
        .collect();
    names.sort();
    for patch in names {
        let text = fs::read_to_string(&patch).unwrap_or_else(|e| panic!("cannot read {}: {e}", patch.display()));
        apply(&out, &text).unwrap_or_else(|e| panic!("{} does not apply to SDL 3.4.18: {e}", patch.display()));
    }
}

fn copy_tree(from: &Path, to: &Path, top: bool) {
    fs::create_dir_all(to).unwrap_or_else(|e| panic!("cannot create {}: {e}", to.display()));
    for entry in fs::read_dir(from).unwrap_or_else(|e| panic!("cannot read {}: {e}", from.display())) {
        let entry = entry.expect("SDL directory entry");
        let name = entry.file_name();
        if name == ".git" || (top && (name == "test" || name == "examples")) {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        let kind = entry.file_type().expect("SDL file type");
        if kind.is_dir() {
            copy_tree(&source, &target, false);
        } else {
            fs::copy(&source, &target).unwrap_or_else(|e| panic!("cannot copy {}: {e}", source.display()));
        }
    }
}

/// Applies the unified diff `patch` to the tree at `root` (paths after `a/` and `b/`).
fn apply(root: &Path, patch: &str) -> Result<(), String> {
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(path) = line.strip_prefix("+++ b/") else { continue };
        let file = root.join(path.trim_end());
        let text = fs::read_to_string(&file).map_err(|e| format!("{path}: {e}"))?;
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut content: Vec<String> = text.lines().map(str::to_owned).collect();
        let mut cursor = 0;
        while lines.peek().is_some_and(|next| next.starts_with("@@")) {
            lines.next();
            let (mut old, mut new) = (Vec::new(), Vec::new());
            while let Some(&body) = lines.peek() {
                match body.chars().next() {
                    Some(' ') => { old.push(&body[1..]); new.push(&body[1..]); }
                    Some('-') if !body.starts_with("--- ") => old.push(&body[1..]),
                    Some('+') if !body.starts_with("+++ ") => new.push(&body[1..]),
                    Some('\\') => {}
                    None => { old.push(""); new.push(""); }
                    _ => break,
                }
                lines.next();
            }
            let last = content.len().checked_sub(old.len()).ok_or_else(|| format!("{path}: a hunk is longer than the file"))?;
            let at = (cursor..=last)
                .find(|&start| content[start..start + old.len()].iter().zip(&old).all(|(have, want)| have == want))
                .ok_or_else(|| format!("{path}: no match for the hunk starting `{}`", old.first().copied().unwrap_or("")))?;
            content.splice(at..at + old.len(), new.iter().map(|line| (*line).to_owned()));
            cursor = at + new.len();
        }
        let mut joined = content.join(eol);
        joined.push_str(eol);
        fs::write(&file, joined).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}
