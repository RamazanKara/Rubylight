//! Tests for profile permission repair, Nucleus copy preservation and untrusted profiles.
use super::super::*;

#[test]
fn upgrades_repair_legacy_profile_ownership_and_preserve_settings() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(profile.join("credentials"))?;
    let settings = profile.join("sunshine.conf");
    std::fs::write(&settings, b"port=48123\n")?;
    let (_, identity) = system::run(
        &system::system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    for path in [&root, &settings] {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/setowner",
                &format!("*{sid}"),
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/grant:r",
                "*S-1-1-0:(F)",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
    }
    assert_eq!(prepare_profile(&profile, true)?, None);
    system::trusted_ancestors(&root)?;
    system::trusted_folder(&settings)?;
    assert_eq!(std::fs::read(&settings)?, b"port=48123\n");
    let credentials = profile.join("credentials");
    for (path, sid) in [
        (&root, "*S-1-1-0"),
        (&settings, "*S-1-1-0"),
        (&credentials, "*S-1-5-32-545"),
    ] {
        let (_, output) = system::run(
            &system::system32("icacls.exe"),
            &[&path.display().to_string(), "/findsid", sid, "/Q"],
            Duration::from_secs(10),
        )?;
        assert!(!output.contains(&path.display().to_string()), "{output}");
    }
    Ok(())
}
#[test]
fn credentials_and_backups_are_restricted_directly_without_resetting_to_users_read() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("Butterpollo").join("config");
    let restricted = std::cell::RefCell::new(Vec::new());
    let inherited = std::cell::RefCell::new(Vec::new());
    let error = secure_profile_with(
        &profile,
        |folder, users_read| {
            assert!(
                inherited.borrow().is_empty(),
                "private folders must be locked before any inherit pass"
            );
            restricted
                .borrow_mut()
                .push((folder.to_owned(), users_read));
            Ok(())
        },
        |folder| {
            inherited.borrow_mut().push(folder.to_owned());
            if folder == profile {
                bail!("resetting permissions failed (5)");
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("resetting permissions failed"));
    assert_eq!(
        *restricted.borrow(),
        [
            (temp.path().join("Butterpollo"), true),
            (profile.clone(), true),
            (profile.join("credentials"), false),
            (profile.join("backups"), false),
        ]
    );
    assert_eq!(
        *inherited.borrow(),
        [
            profile.join("credentials"),
            profile.join("backups"),
            profile.clone(),
            temp.path().join("Butterpollo"),
        ]
    );
    Ok(())
}
#[test]
fn legacy_profile_repair_rejects_reparse_points_and_untrusted_parents() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    let settings = outside.join("sunshine.conf");
    std::fs::write(&settings, b"unchanged")?;
    let linked = temp.path().join("linked");
    std::os::windows::fs::symlink_dir(&outside, &linked)?;
    for profile in [linked.join("config"), linked.join("Butterpollo/config")] {
        let error = prepare_profile(&profile, true).unwrap_err();
        assert!(error.to_string().contains("reparse point"), "{error:#}");
    }
    let profile = temp.path().join("Butterpollo/config");
    std::fs::create_dir_all(&profile)?;
    std::os::windows::fs::symlink_dir(&outside, profile.join("credentials"))?;
    let error = prepare_profile(&profile, true).unwrap_err();
    assert!(error.to_string().contains("reparse point"), "{error:#}");
    assert_eq!(std::fs::read(&settings)?, b"unchanged");
    assert!(!outside.join("config").exists());
    assert!(!outside.join("Butterpollo").exists());
    let untrusted = temp.path().join("untrusted");
    std::fs::create_dir(&untrusted)?;
    // Not an administrator's: an administrator's account may own a parent.
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &untrusted.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    for updating in [true, false] {
        let error = prepare_profile(&untrusted.join("Butterpollo/config"), updating).unwrap_err();
        assert!(error.to_string().contains("must be owned by"), "{error:#}");
        assert!(!untrusted.join("Butterpollo").exists());
    }
    Ok(())
}
/// `icacls` on `path` with `args`, which must succeed.
fn icacls(path: &Path, args: &[&str]) -> Result<()> {
    let path = path.display().to_string();
    let args: Vec<&str> = [path.as_str()]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &args,
        Duration::from_secs(10),
    )?;
    anyhow::ensure!(code == 0, "icacls {args:?}: {output}");
    Ok(())
}
/// Whether `path`'s permissions name `sid`.
fn names(path: &Path, sid: &str) -> Result<bool> {
    let (_, output) = system::run(
        &system::system32("icacls.exe"),
        &[&path.display().to_string(), "/findsid", sid, "/Q"],
        Duration::from_secs(10),
    )?;
    Ok(output.contains(&path.display().to_string()))
}
/// `path`'s permissions as icacls lists them.
fn acl(path: &Path) -> Result<String> {
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[&path.display().to_string()],
        Duration::from_secs(10),
    )?;
    anyhow::ensure!(code == 0, "{output}");
    // icacls aligns its lines with the name.
    let output = output.replace(&path.display().to_string(), "");
    Ok(output.split_whitespace().collect::<Vec<_>>().join(" "))
}
/// A user's Nucleus copy below `root`, in folders with the permissions
/// the host gives them (`user_copy.rs`), Users standing in for the user:
/// they may write in the copy, and own what they put there. With
/// `link_to`, that includes a junction to it.
fn nucleus_copy(root: &Path, link_to: Option<&Path>) -> Result<PathBuf> {
    let nucleus = nucleus_user_root(root);
    // The host's `ROOT`.
    system::create_restricted(&nucleus, false)?;
    let folder = nucleus.join("S-1-5-21-1");
    let copy = folder.join("0123456789abcdef");
    std::fs::create_dir(&folder)?;
    std::fs::create_dir(&copy)?;
    for (path, user) in [
        (&folder, "*S-1-5-32-545:(RX)"),
        (&copy, "*S-1-5-32-545:(OI)(CI)(M)"),
    ] {
        icacls(
            path,
            &[
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)(F)",
                "*S-1-5-32-544:(OI)(CI)(F)",
                user,
            ],
        )?;
    }
    // Named like what setup resets in the profile: a file icacls would
    // also look for in every folder below, and a folder.
    std::fs::write(copy.join("handler.js"), b"user's")?;
    std::fs::create_dir(copy.join("config"))?;
    icacls(&copy.join("handler.js"), &["/setowner", "*S-1-5-32-545"])?;
    icacls(&copy.join("config"), &["/setowner", "*S-1-5-32-545"])?;
    if let Some(target) = link_to {
        let (code, output) = system::run(
            &system::system32("cmd.exe"),
            &[
                "/c",
                "mklink",
                "/J",
                &copy.join("linked").display().to_string(),
                &target.display().to_string(),
            ],
            Duration::from_secs(10),
        )?;
        anyhow::ensure!(code == 0, "mklink: {output}");
    }
    Ok(copy)
}
/// The copy and its folders still have the host's permissions: setup
/// changed none of them.
fn assert_nucleus_copy_kept(root: &Path, copy: &Path) -> Result<()> {
    let nucleus = nucleus_user_root(root);
    // Users may not even list the folder of everyone's copies.
    assert!(!names(&nucleus, "*S-1-5-32-545")?);
    assert!(names(copy, "*S-1-5-32-545")?);
    for theirs in [copy.join("handler.js"), copy.join("config")] {
        let error = system::trusted_folder(&theirs).unwrap_err();
        assert!(
            error.to_string().contains("must be owned by"),
            "{}: {error:#}",
            theirs.display()
        );
    }
    assert_eq!(std::fs::read(copy.join("handler.js"))?, b"user's");
    Ok(())
}
#[test]
fn reinstalls_leave_the_users_nucleus_copies_as_the_host_made_them() -> Result<()> {
    // A reinstall reset every Nucleus copy to the profile's permissions,
    // so the host could no longer write the user's copy as the user, and
    // a junction the user made in theirs failed every reinstall.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    system::create_restricted(&root, true)?;
    std::fs::create_dir_all(profile.join("credentials"))?;
    std::fs::write(root.join("handler.js"), b"setup's")?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sunshine.conf"), b"unchanged")?;
    let copy = nucleus_copy(&root, Some(&outside))?;
    let nucleus = nucleus_user_root(&root);
    let linked = [outside.clone(), outside.join("sunshine.conf")];
    let acls = || {
        linked
            .iter()
            .map(|path| acl(path))
            .collect::<Result<Vec<_>>>()
    };
    let before = acls()?;
    // An update, then a reinstall's or a full installation's steps.
    for installed in [true, false] {
        assert_eq!(prepare_profile(&profile, installed)?, None);
    }
    secure_profile(&profile)?;
    assert_nucleus_copy_kept(&root, &copy)?;
    system::trusted_tree(&root, std::slice::from_ref(&nucleus))?;
    assert_eq!(std::fs::read(outside.join("sunshine.conf"))?, b"unchanged");
    assert_eq!(acls()?, before);
    // The copies' folder itself is still checked: a link in its place
    // is refused, and setup leaves it as it is.
    std::fs::remove_dir_all(&nucleus)?;
    std::os::windows::fs::symlink_dir(&outside, &nucleus)?;
    for error in [
        secure_profile(&profile).unwrap_err(),
        prepare_profile(&profile, true).unwrap_err(),
    ] {
        assert!(error.to_string().contains("reparse point"), "{error:#}");
    }
    assert!(
        std::fs::symlink_metadata(&nucleus)?
            .file_type()
            .is_symlink()
    );
    assert_eq!(acls()?, before);
    std::fs::remove_dir(&nucleus)?;
    // So is one users may change: the repair leaves it, and fails.
    std::fs::create_dir(&nucleus)?;
    icacls(&nucleus, &["/grant", "*S-1-5-32-545:(M)"])?;
    let error = format!("{:#}", prepare_profile(&profile, true).unwrap_err());
    for said in [
        format!("an administrator can delete {}", nucleus.display()),
        format!("{} may be changed by", nucleus.display()),
    ] {
        assert!(error.contains(&said), "{error}");
    }
    assert!(names(&nucleus, "*S-1-5-32-545")?);
    Ok(())
}
#[test]
fn a_legacy_repair_that_failed_partway_is_finished_by_the_next_run() -> Result<()> {
    // The repair ran only while the root had another owner: once it got
    // past the root, what it had not reached yet stayed the user's.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(profile.join("credentials"))?;
    let settings = profile.join("sunshine.conf");
    std::fs::write(&settings, b"port=48123\n")?;
    let copy = nucleus_copy(&root, None)?;
    let nucleus = nucleus_user_root(&root);
    // As earlier installers left it.
    for path in [&root, &settings] {
        icacls(path, &["/setowner", "*S-1-5-32-545"])?;
    }
    icacls(&settings, &["/grant", "*S-1-5-32-545:(M)"])?;
    let error = secure_profile_with(
        &profile,
        |folder, users_read| {
            if std::fs::symlink_metadata(folder).is_err() {
                system::create_restricted(folder, users_read)
            } else {
                system::restrict_except(folder, users_read, std::slice::from_ref(&nucleus))
            }
        },
        |_| bail!("resetting permissions failed (5)"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("resetting permissions failed"));
    // The root is the service's now, its settings still the user's.
    system::trusted_parents(&profile)?;
    assert!(system::trusted_tree(&root, std::slice::from_ref(&nucleus)).is_err());
    // Every later run repairs until all of it is the service's, leaving
    // the Nucleus copies as they are.
    for _ in 0..2 {
        assert_eq!(prepare_profile(&profile, true)?, None);
        system::trusted_tree(&root, std::slice::from_ref(&nucleus))?;
        assert_nucleus_copy_kept(&root, &copy)?;
    }
    assert_eq!(std::fs::read(&settings)?, b"port=48123\n");
    Ok(())
}
#[test]
fn an_installed_profile_gone_after_setup_looked_is_an_error() -> Result<()> {
    // The repair found nothing to check and went on; the update then
    // made the profile with the permissions ProgramData gives everyone.
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("Butterpollo");
    let error = repair_profile(&root.join("config")).unwrap_err();
    assert!(
        format!("{error:#}").starts_with(&format!("checking {}", root.display())),
        "{error:#}"
    );
    assert!(!root.exists());
    Ok(())
}
#[test]
fn fresh_installs_move_an_untrusted_profile_aside_unchanged() -> Result<()> {
    // #61: a ProgramData\Butterpollo that the user owned, with no
    // Rubylight installed, stopped every fresh install.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(&profile)?;
    std::fs::write(profile.join("sunshine.conf"), b"planted")?;
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &root.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    let aside = prepare_profile(&profile, false)?.context("the profile stayed")?;
    // A new root, locked down and empty, replaces it.
    system::trusted_folder(&root)?;
    assert_eq!(std::fs::read_dir(&root)?.count(), 0);
    assert_eq!(aside.parent(), Some(temp.path()));
    assert!(
        aside
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("Butterpollo.untrusted-")),
        "{}",
        aside.display()
    );
    assert_eq!(
        std::fs::read(aside.join("config/sunshine.conf"))?,
        b"planted"
    );
    assert!(system::trusted_folder(&aside).is_err());
    // A trusted profile stays where it is.
    std::fs::create_dir_all(&profile)?;
    assert_eq!(prepare_profile(&profile, false)?, None);
    assert!(profile.is_dir());
    // So is a trusted root holding something someone else owns: its owner
    // could change its permissions back.
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &profile.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    let child_aside = prepare_profile(&profile, false)?.context("the child stayed")?;
    // A second move, usually within the same second, gets its own name.
    assert_ne!(child_aside, aside);
    assert!(child_aside.join("config").is_dir());
    assert_eq!(std::fs::read_dir(&root)?.count(), 0);
    // A missing root is made locked down from the start, also for an
    // installed service whose profile was deleted.
    for installed in [false, true] {
        std::fs::remove_dir_all(&root)?;
        assert_eq!(prepare_profile(&profile, installed)?, None);
        system::trusted_tree(&root, &[])?;
    }
    // Something users may change or that they own moves aside too.
    let icacls = |args: &[&str]| -> Result<()> {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            args,
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        Ok(())
    };
    let profile_text = profile.display().to_string();
    for change in [
        ["/grant", "*S-1-5-32-545:(OI)(CI)(M)"],
        ["/setowner", "*S-1-5-32-545"],
    ] {
        std::fs::create_dir(&profile)?;
        icacls(&[&profile_text, change[0], change[1], "/Q"])?;
        let error = system::trusted_tree(&root, &[]).unwrap_err();
        assert!(
            error.downcast_ref::<system::Untrusted>().is_some(),
            "{error:#}"
        );
        assert!(prepare_profile(&profile, false)?.is_some());
        assert!(!profile.exists());
    }
    // The host lets users write in their Nucleus copies and checks them
    // itself: a profile holding one stays, unless the copies' folder
    // itself is open to users.
    let nucleus = nucleus_user_root(&root);
    assert_eq!(
        nucleus,
        butterpollo_session::nucleus::install::user_root(&root)
    );
    let copy = nucleus.join("S-1-5-21-1").join("0123456789abcdef");
    std::fs::create_dir_all(&copy)?;
    std::fs::write(copy.join("handler.js"), b"user's")?;
    let copy_text = copy.display().to_string();
    icacls(&[&copy_text, "/grant", "*S-1-5-32-545:(OI)(CI)(M)", "/Q"])?;
    icacls(&[&copy_text, "/setowner", "*S-1-5-32-545", "/T", "/Q"])?;
    assert_eq!(prepare_profile(&profile, false)?, None);
    assert_eq!(std::fs::read(copy.join("handler.js"))?, b"user's");
    let nucleus_text = nucleus.display().to_string();
    icacls(&[&nucleus_text, "/grant", "*S-1-5-32-545:(M)", "/Q"])?;
    assert!(prepare_profile(&profile, false)?.is_some());
    assert!(!nucleus.exists());
    Ok(())
}
#[test]
fn folders_created_restricted_match_restricted_ones() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    for users_read in [true, false] {
        let created = temp.path().join(format!("created-{users_read}"));
        let restricted = temp.path().join(format!("restricted-{users_read}"));
        system::create_restricted(&created, users_read)?;
        assert!(system::create_restricted(&created, users_read).is_err());
        std::fs::create_dir(&restricted)?;
        system::restrict(&restricted, users_read)?;
        assert_eq!(acl(&created)?, acl(&restricted)?);
        system::trusted_tree(&created, &[])?;
    }
    Ok(())
}
#[test]
fn fresh_installs_move_a_linked_profile_aside_not_its_target() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sunshine.conf"), b"unchanged")?;
    let linked = temp.path().join("linked");
    std::os::windows::fs::symlink_dir(&outside, &linked)?;
    let aside = prepare_profile(&linked.join("config"), false)?.context("the link stayed")?;
    assert!(std::fs::symlink_metadata(&aside)?.file_type().is_symlink());
    assert!(!std::fs::symlink_metadata(&linked)?.file_type().is_symlink());
    system::trusted_folder(&linked)?;
    assert_eq!(std::fs::read(outside.join("sunshine.conf"))?, b"unchanged");
    assert_eq!(std::fs::read_dir(&outside)?.count(), 1);
    Ok(())
}
#[test]
fn inherit_contents_sets_admin_owner_before_resetting_permissions() -> Result<()> {
    assert_eq!(
        system::INHERIT_OPERATIONS[0].0,
        &["/setowner", "*S-1-5-32-544"]
    );
    assert_eq!(system::INHERIT_OPERATIONS[1].0, &["/reset"]);
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let folder = temp.path().join("restricted");
    system::create_restricted(&folder, true)?;
    let child = folder.join("locked.conf");
    std::fs::write(&child, b"secret")?;
    // Strip Administrators from the child's ACL and make Users its owner so
    // `/reset` would fail with ACCESS_DENIED unless `/setowner` runs first.
    icacls(&child, &["/setowner", "*S-1-5-32-545"])?;
    icacls(&child, &["/inheritance:r", "/grant:r", "*S-1-5-32-545:(F)"])?;
    system::inherit_contents(&folder, &[])?;
    system::trusted_folder(&child)?;
    assert_eq!(std::fs::read(&child)?, b"secret");
    Ok(())
}
