//! Copies the SDL submodule into OUT_DIR (without .git, test and examples, as the crates.io
//! crate ships it) and applies every patch in patches/ there; the submodule stays pristine. The
//! patches are unified diffs applied hunk by hunk at their exact context, in order: a hunk that
//! does not match fails the build, so a patch never applies partly or silently, and so does a
//! patch that changes nothing (no `+++ b/` file, a file without hunks, paths without `a/` `b/`).
//! The tests run without Cargo: `rustc --edition 2021 --test build.rs -o /tmp/sdl3-src-build-test && /tmp/sdl3-src-build-test`.

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
        apply(&out, &text).unwrap_or_else(|e| panic!("{} does not apply to SDL 3.4.20: {e}", patch.display()));
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

/// Applies the unified diff `patch` to the tree at `root` (paths after `a/` and `b/`) and returns
/// how many files and hunks it changed. A patch that changes nothing is an error.
fn apply(root: &Path, patch: &str) -> Result<(usize, usize), String> {
    let (mut files, mut hunks) = (0, 0);
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(target) = line.strip_prefix("+++ ") else { continue };
        let path = target.strip_prefix("b/").ok_or_else(|| format!("`{line}`: the paths need the a/ and b/ prefixes (diff -u with a/ and b/, not --no-prefix)"))?;
        let file = root.join(path.trim_end());
        let text = fs::read_to_string(&file).map_err(|e| format!("{path}: {e}"))?;
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut content: Vec<String> = text.lines().map(str::to_owned).collect();
        let (mut cursor, mut file_hunks) = (0, 0);
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
            file_hunks += 1;
        }
        if file_hunks == 0 {
            return Err(format!("{path}: no hunks follow its header"));
        }
        let mut joined = content.join(eol);
        joined.push_str(eol);
        fs::write(&file, joined).map_err(|e| format!("{path}: {e}"))?;
        files += 1;
        hunks += file_hunks;
    }
    if files == 0 {
        return Err("no file in it (no `+++ b/` header): it would change nothing".into());
    }
    Ok((files, hunks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sdl3-src-patch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.c"), "one\ntwo\nthree\n").unwrap();
        root
    }

    #[test]
    fn a_patch_applies_and_counts_its_files_and_hunks() {
        let root = tree("ok");
        let patch = "--- a/src/a.c\n+++ b/src/a.c\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n";
        assert_eq!(apply(&root, patch), Ok((1, 1)));
        assert_eq!(fs::read_to_string(root.join("src/a.c")).unwrap(), "one\n2\nthree\n");
    }

    #[test]
    fn a_patch_that_changes_nothing_fails() {
        let root = tree("nothing");
        // --no-prefix: no `+++ b/` header, which used to apply nothing and pass.
        let error = apply(&root, "--- src/a.c\n+++ src/a.c\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n").unwrap_err();
        assert!(error.contains("a/ and b/"), "{error}");
        assert!(apply(&root, "").unwrap_err().contains("no file"));
        assert!(apply(&root, "just some text\n").unwrap_err().contains("no file"));
        assert!(apply(&root, "--- a/src/a.c\n+++ b/src/a.c\n").unwrap_err().contains("no hunks"));
        assert_eq!(fs::read_to_string(root.join("src/a.c")).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn a_hunk_that_does_not_match_fails() {
        let root = tree("mismatch");
        let error = apply(&root, "--- a/src/a.c\n+++ b/src/a.c\n@@ -1,3 +1,3 @@\n one\n-zwei\n+2\n three\n").unwrap_err();
        assert!(error.contains("no match"), "{error}");
    }
}
