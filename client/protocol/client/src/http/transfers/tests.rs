use super::*;

fn refused(refusal: FileRefusal) -> FileError {
    FileError::Refused {
        refusal,
        status: 403,
        message: format!("Host says {}", refusal.code()),
    }
}

#[test]
fn every_refusal_has_its_problem() {
    let table = [
        (FileRefusal::Forbidden, Problem::NoPermission),
        (FileRefusal::FileTransferDisabled, Problem::TransferOff),
        (FileRefusal::InvalidName, Problem::InvalidName),
        (FileRefusal::IncompleteUpload, Problem::Interrupted),
        (FileRefusal::LengthRequired, Problem::Host),
        (FileRefusal::TooLarge, Problem::TooLarge),
        (FileRefusal::InsufficientStorage, Problem::NoSpace),
        (FileRefusal::NoConsoleUser, Problem::NoConsoleUser),
        (FileRefusal::TransferBusy, Problem::Busy),
        (FileRefusal::NotFound, Problem::NotFound),
        (FileRefusal::FoldersDisabled, Problem::Host),
        (FileRefusal::FolderUnreadable, Problem::Host),
    ];
    for (refusal, problem) in table {
        assert_eq!(Problem::of(&refused(refusal)), Some(problem), "{refusal:?}");
    }
    let io = || std::io::Error::other("reset");
    assert_eq!(Problem::of(&FileError::Local(io())), Some(Problem::Local));
    assert_eq!(
        Problem::of(&FileError::Host(Error::Io(io()))),
        Some(Problem::Network)
    );
    assert_eq!(Problem::of(&FileError::Host(Error::Cancelled)), None);
    for other in [
        Error::CertificateMismatch,
        Error::Malformed("the download ended after 5 of 9 bytes".into()),
        Error::Status {
            code: 500,
            message: "x".into(),
        },
        Error::NotFound("/files".into()),
    ] {
        assert_eq!(Problem::of(&FileError::Host(other)), Some(Problem::Host));
    }
}

#[test]
fn problem_ids_are_stable_and_unique() {
    let ids: Vec<&str> = Problem::ALL.iter().map(|p| p.id()).collect();
    assert_eq!(
        ids,
        [
            "no_permission",
            "transfer_off",
            "invalid_name",
            "interrupted",
            "too_large",
            "no_space",
            "no_console_user",
            "busy",
            "not_found",
            "network",
            "local",
            "host"
        ]
    );
    for problem in Problem::ALL {
        assert_eq!(Problem::from_id(problem.id()), Some(problem));
    }
    assert_eq!(Problem::from_id("forbidden"), None);
    // Only the permission and the setting are fixed in the console.
    let console: Vec<Problem> = Problem::ALL
        .into_iter()
        .filter(|p| p.console_fix())
        .collect();
    assert_eq!(console, [Problem::NoPermission, Problem::TransferOff]);
    let final_: Vec<Problem> = Problem::ALL
        .into_iter()
        .filter(|p| !p.retryable())
        .collect();
    assert_eq!(
        final_,
        [Problem::InvalidName, Problem::TooLarge, Problem::NotFound]
    );
}

#[test]
fn the_host_limits_decide_what_starts() {
    let mut list = Transfers::new();
    let up1 = list.queue_upload("a.txt", 10);
    let up2 = list.queue_upload("b.txt", 10);
    let down1 = list.queue_download("c.txt", "c.txt", 10);
    let down2 = list.queue_download("d.txt", "d.txt", 10);
    let down3 = list.queue_download("e.txt", "e.txt", 10);
    let started: Vec<u64> = list.start_next().iter().map(|s| s.id).collect();
    assert_eq!(started, [up1, down1, down2]);
    // Nothing more until one finishes.
    assert!(list.start_next().is_empty());
    list.finish_download(down1, Ok(10));
    assert_eq!(
        list.start_next().iter().map(|s| s.id).collect::<Vec<_>>(),
        [down3]
    );
    list.finish_upload(
        up1,
        Ok(Uploaded {
            name: "a (1).txt".into(),
            size: 10,
        }),
    );
    let next = list.start_next();
    assert_eq!(
        (next.len(), next[0].id, next[0].name.as_str()),
        (1, up2, "b.txt")
    );
    assert_eq!(
        list.get(up1).unwrap().state,
        State::Done {
            name: "a (1).txt".into(),
            size: 10
        }
    );
    assert!(list.uploaded_since(&[up1]));
    assert!(!list.uploaded_since(&[up2, down1]));
}

#[test]
fn progress_only_moves_forward_while_running() {
    let mut list = Transfers::new();
    let id = list.queue_download("big.bin", "big.bin", 0);
    assert!(!list.progress(id, 5, 100), "queued");
    list.start_next();
    assert_eq!(list.get(id).unwrap().fraction(), Some(0.0));
    assert!(list.progress(id, 0, 100));
    assert_eq!(
        list.get(id).unwrap().size,
        100,
        "a download learns its size"
    );
    assert!(list.progress(id, 64 * 1024, 100 * 1024));
    assert!(!list.progress(id, 64 * 1024, 100 * 1024), "the same report");
    assert!(!list.progress(id, 16 * 1024, 100 * 1024), "backwards");
    assert_eq!(list.get(id).unwrap().fraction(), Some(0.64));
    list.finish_download(id, Ok(100 * 1024));
    assert!(!list.progress(id, 100 * 1024, 100 * 1024), "finished");
    assert_eq!(list.get(id).unwrap().fraction(), Some(1.0));
    assert!(!list.progress(99, 1, 1), "unknown");
}

#[test]
fn failures_keep_their_problem_and_message() {
    let mut list = Transfers::new();
    let id = list.queue_upload("a.txt", 10);
    list.start_next();
    list.finish_upload(id, Err(refused(FileRefusal::Forbidden)));
    let t = list.get(id).unwrap();
    assert_eq!(
        t.state,
        State::Failed {
            problem: Problem::NoPermission,
            message: "host error 403: Host says forbidden".into()
        }
    );
    assert!(t.can_retry());
    assert!(list.retry(id));
    assert_eq!(list.get(id).unwrap().state, State::Queued);
    list.start_next();
    list.finish_upload(id, Err(refused(FileRefusal::TooLarge)));
    assert!(!list.get(id).unwrap().can_retry());
    assert!(!list.retry(id), "too large stays too large");
    // An answer for a transfer that is not running changes nothing.
    list.finish_upload(
        id,
        Ok(Uploaded {
            name: "a.txt".into(),
            size: 10,
        }),
    );
    assert!(matches!(
        list.get(id).unwrap().state,
        State::Failed {
            problem: Problem::TooLarge,
            ..
        }
    ));
}

#[test]
fn a_cancel_stops_queued_at_once_and_running_through_the_worker() {
    let mut list = Transfers::new();
    let running = list.queue_upload("a.txt", 10);
    let queued = list.queue_upload("b.txt", 10);
    let start = list.start_next().remove(0);
    assert!(list.cancel(queued));
    assert_eq!(list.get(queued).unwrap().state, State::Cancelled);
    assert!(list.cancel(running));
    assert!(start.cancel.is_cancelled(), "the worker's handle");
    assert!(list.get(running).unwrap().is_cancelling());
    assert!(!list.cancel(running), "once");
    assert!(list.start_next().is_empty(), "the slot is still taken");
    // Whatever the worker failed with after the cancel, it was cancelled.
    list.finish_upload(
        running,
        Err(FileError::Host(Error::Io(std::io::Error::other(
            "shut down",
        )))),
    );
    assert_eq!(list.get(running).unwrap().state, State::Cancelled);
    assert!(!list.get(running).unwrap().is_cancelling());
    // A retry gets a fresh handle: the old one cancels every transfer started with it.
    assert!(list.retry(running));
    let again = list.start_next().remove(0);
    assert_eq!(again.id, running);
    assert!(!again.cancel.is_cancelled());
    // One that finished before the cancel took hold is done.
    list.cancel(running);
    list.finish_upload(
        running,
        Ok(Uploaded {
            name: "a.txt".into(),
            size: 10,
        }),
    );
    assert!(matches!(
        list.get(running).unwrap().state,
        State::Done { .. }
    ));
}

#[test]
fn cancel_all_and_clear_finished() {
    let mut list = Transfers::new();
    let a = list.queue_download("a", "a", 1);
    let b = list.queue_download("b", "b", 1);
    let c = list.queue_download("c", "c", 1);
    list.start_next();
    list.finish_download(a, Ok(1));
    assert!(list.is_active());
    list.cancel_all();
    assert_eq!(list.get(c).unwrap().state, State::Cancelled);
    assert!(list.get(b).unwrap().is_cancelling());
    list.finish_download(b, Err(FileError::Host(Error::Cancelled)));
    assert!(!list.is_active());
    list.clear_finished();
    assert!(list.items().is_empty());
    // Ids are never reused.
    assert_eq!(list.queue_upload("d", 1), c + 1);
}

#[test]
fn a_cancel_error_without_a_cancel_request_is_still_cancelled() {
    let mut list = Transfers::new();
    let id = list.queue_upload("a", 1);
    list.start_next();
    list.finish_upload(id, Err(FileError::Host(Error::Cancelled)));
    assert_eq!(list.get(id).unwrap().state, State::Cancelled);
}

#[test]
fn upload_names_are_ones_the_host_takes() {
    // The host's own check of these names runs in its interop tests, which build this crate.
    for (local, sent) in [
        ("photo.jpg", "photo.jpg"),
        (
            "Screenshot 2026-10-27 at 10:15:00.png",
            "Screenshot 2026-10-27 at 10_15_00.png",
        ),
        ("a<b>c\"d|e?f*g.txt", "a_b_c_d_e_f_g.txt"),
        ("tab\there.txt", "tab_here.txt"),
        ("\u{202e}txt.exe", "txt.exe"),
        ("trailing. . ", "trailing"),
        ("CON.txt", "_CON.txt"),
        ("nul", "_nul"),
        ("lpt¹.log", "_lpt¹.log"),
        ("movie.mp4.part", "movie.mp4.part_"),
        ("MOVIE.PART", "MOVIE.PART_"),
        ("/storage/emulated/0/DCIM/clip.mp4", "clip.mp4"),
        ("C:\\Users\\me\\clip.mp4", "clip.mp4"),
        ("", "file"),
        ("...", "file"),
        ("..", "file"),
        ("emoji 👨‍👩‍👧.png", "emoji 👨‍👩‍👧.png"),
    ] {
        assert_eq!(upload_name(local), sent, "{local:?}");
    }
}

#[test]
fn long_names_keep_their_extension() {
    let long = format!("{}.mkv", "x".repeat(300));
    let name = upload_name(&long);
    assert_eq!(name.encode_utf16().count(), MAX_NAME_UNITS);
    assert!(name.ends_with("xx.mkv"));
    // Characters outside the BMP count two units and are never split.
    let wide = format!("{}.txt", "😀".repeat(150));
    let name = upload_name(&wide);
    assert!(name.encode_utf16().count() <= MAX_NAME_UNITS);
    assert!(name.ends_with("😀.txt"));
    // A long "extension" is part of the name.
    let dotted = format!("a.{}", "y".repeat(300));
    assert_eq!(upload_name(&dotted).encode_utf16().count(), MAX_NAME_UNITS);
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nightjar-transfers-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("saved.bin")
}

#[test]
fn a_download_lands_whole_or_not_at_all() {
    let dest = scratch("whole");
    std::fs::write(&dest, b"old").unwrap();
    // Cut short: the old file stays and the partial one is gone.
    let result = download_into(&dest, |sink| {
        sink.write_all(b"new but sh").unwrap();
        Err(FileError::Host(Error::Malformed(
            "the download ended after 10 of 20 bytes".into(),
        )))
    });
    assert!(matches!(result, Err(FileError::Host(Error::Malformed(_)))));
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert!(!partial_path(&dest).exists());
    // Cancelled: the same.
    let result = download_into(&dest, |sink| {
        sink.write_all(b"x").unwrap();
        Err(FileError::Host(Error::Cancelled))
    });
    assert!(matches!(result, Err(FileError::Host(Error::Cancelled))));
    assert!(!partial_path(&dest).exists());
    // Whole: it takes the free name.
    std::fs::remove_file(&dest).unwrap();
    let result = download_into(&dest, |sink| {
        sink.write_all(b"new and whole").unwrap();
        Ok(13)
    });
    assert_eq!(result.unwrap(), 13);
    assert_eq!(std::fs::read(&dest).unwrap(), b"new and whole");
    assert!(!partial_path(&dest).exists());
    std::fs::remove_dir_all(dest.parent().unwrap()).unwrap();
}

#[test]
fn a_file_that_takes_the_name_during_a_download_is_not_replaced() {
    let dest = scratch("taken");
    let result = download_into(&dest, |sink| {
        sink.write_all(b"downloaded").unwrap();
        // A browser saves its own file under the name meanwhile.
        std::fs::write(&dest, b"the user's").unwrap();
        Ok(10)
    });
    match result {
        Err(FileError::Local(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{e}");
            assert!(e.to_string().contains("was not replaced"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&dest).unwrap(), b"the user's");
    assert!(!partial_path(&dest).exists());
    std::fs::remove_dir_all(dest.parent().unwrap()).unwrap();
}

#[test]
fn a_part_file_that_is_already_there_is_left_alone() {
    let dest = scratch("part");
    let part = partial_path(&dest);
    std::fs::write(&part, b"a browser's download").unwrap();
    let mut called = false;
    let result = download_into(&dest, |_| {
        called = true;
        Ok(0)
    });
    assert!(
        matches!(&result, Err(FileError::Local(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{result:?}"
    );
    assert!(!called);
    assert_eq!(std::fs::read(&part).unwrap(), b"a browser's download");
    assert!(!dest.exists());
    std::fs::remove_dir_all(dest.parent().unwrap()).unwrap();
}

#[test]
fn a_partial_file_that_cannot_be_deleted_is_named_in_the_error() {
    let dest = scratch("stays");
    let part = partial_path(&dest);
    let result = download_into(&dest, |_| {
        // Swap the open file's name for a directory, which remove_file cannot delete.
        std::fs::remove_file(&part).unwrap();
        std::fs::create_dir(&part).unwrap();
        Err(FileError::Host(Error::Cancelled))
    });
    match result {
        Err(FileError::Local(e)) => {
            let text = e.to_string();
            assert!(
                text.contains("saved.bin.part") && text.contains("could not be deleted"),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
    std::fs::remove_dir_all(dest.parent().unwrap()).unwrap();
}

#[test]
fn a_download_into_a_missing_folder_is_a_local_failure() {
    let dest = std::env::temp_dir()
        .join("nightjar-no-such-folder-7f3a")
        .join("x.bin");
    let mut called = false;
    let result = download_into(&dest, |_| {
        called = true;
        Ok(0)
    });
    assert!(matches!(result, Err(FileError::Local(_))));
    assert!(!called, "nothing is fetched without a place to put it");
}

#[test]
fn the_partial_file_sits_beside_the_destination() {
    assert_eq!(
        partial_path(Path::new("/home/u/Downloads/clip (1).mp4")),
        Path::new("/home/u/Downloads/clip (1).mp4.part")
    );
}
