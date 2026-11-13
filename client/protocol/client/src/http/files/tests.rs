use super::*;
use crate::http::native_api::error::ErrorCode;

fn response(status: u16, body: &str) -> transport::Response {
    transport::Response {
        status,
        reason: "Reason".into(),
        content_type: Some("application/json".into()),
        body: body.as_bytes().to_vec(),
    }
}

fn error(status: u16, code: &str) -> transport::Response {
    response(
        status,
        &format!(r#"{{"error":{{"code":"{code}","message":"Host says {code}"}}}}"#),
    )
}

#[test]
fn a_download_takes_no_more_than_its_listed_size() {
    let mut file = Vec::new();
    let mut sink = Local::new(Listed {
        inner: &mut file,
        left: 5,
        over: false,
    });
    sink.write_all(b"abc").unwrap();
    sink.write_all(b"de").unwrap();
    assert!(!sink.inner.over, "exactly the listed size is fine");
    let error = sink.write_all(b"f").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(sink.inner.over);
    drop(sink);
    assert_eq!(file, b"abcde", "nothing past the listed size is written");
}

/// The design's error tables, Files and Shared folders, row by row.
const TABLE: [(u16, &str, FileRefusal); 12] = [
    (403, "forbidden", FileRefusal::Forbidden),
    (
        403,
        "file_transfer_disabled",
        FileRefusal::FileTransferDisabled,
    ),
    (400, "invalid_name", FileRefusal::InvalidName),
    (400, "incomplete_upload", FileRefusal::IncompleteUpload),
    (411, "length_required", FileRefusal::LengthRequired),
    (413, "too_large", FileRefusal::TooLarge),
    (
        507,
        "insufficient_storage",
        FileRefusal::InsufficientStorage,
    ),
    (409, "no_console_user", FileRefusal::NoConsoleUser),
    (409, "transfer_busy", FileRefusal::TransferBusy),
    (404, "not_found", FileRefusal::NotFound),
    (403, "folders_disabled", FileRefusal::FoldersDisabled),
    (403, "folder_unreadable", FileRefusal::FolderUnreadable),
];

#[test]
fn every_documented_refusal_is_typed_for_every_call() {
    for (status, code, expected) in TABLE {
        let answers: [Result<(), FileError>; 3] = [
            decode::<FileList>(error(status, code), LIST_PATH, 200).map(drop),
            decode::<Uploaded>(error(status, code), LIST_PATH, 201).map(drop),
            downloaded(error(status, code), CONTENT_PATH, 0).map(drop),
        ];
        for answer in answers {
            match answer {
                Err(FileError::Refused {
                    refusal,
                    status: got,
                    message,
                }) => {
                    assert_eq!((refusal, got), (expected, status), "{code}");
                    assert_eq!(message, format!("Host says {code}"));
                }
                other => panic!("{status} {code}: {other:?}"),
            }
        }
        assert_eq!(expected.code(), code);
    }
}

#[test]
fn fields_the_spec_does_not_name_are_ignored() {
    // The native error body's optional fields; a busy host gives no retry time.
    let body = r#"{"error":{"code":"transfer_busy","message":"busy","setting":null,"retry_after_ms":1500}}"#;
    match decode::<Uploaded>(response(409, body), LIST_PATH, 201) {
        Err(FileError::Refused {
            refusal: FileRefusal::TransferBusy,
            status: 409,
            message,
        }) => assert_eq!(message, "busy"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_undocumented_answer_is_a_loud_host_error() {
    // A known code under another status, a code from elsewhere in the API, and no JSON.
    let answers = [
        (error(403, "not_found"), Some(ErrorCode::NotFound)),
        (error(400, "too_large"), Some(ErrorCode::Unknown)),
        (error(409, "session_busy"), Some(ErrorCode::SessionBusy)),
        (error(500, "internal"), Some(ErrorCode::Internal)),
        (error(418, "teapot"), Some(ErrorCode::Unknown)),
        (response(500, "<html>oops</html>"), None),
        (response(413, ""), None),
    ];
    for (answer, code) in answers {
        let status = answer.status;
        let error = decode::<Uploaded>(answer, LIST_PATH, 201).unwrap_err();
        match (&error, code) {
            (FileError::Host(Error::Api { status: got, error }), Some(code)) => {
                assert_eq!((*got, error.code), (status, code));
            }
            (FileError::Host(Error::Status { code, .. }), None) => {
                assert_eq!(*code, i32::from(status));
            }
            _ => panic!("{status}: {error:?}"),
        }
    }
    // A 404 without the native error body names the path.
    match downloaded(response(404, ""), CONTENT_PATH, 0) {
        Err(FileError::Host(Error::NotFound(path))) => {
            assert_eq!(path, format!("/{CONTENT_PATH}"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_upload_needs_201_and_its_answer() {
    let body = r#"{"name":"photo (1).jpg","size":5}"#;
    assert_eq!(
        uploaded(response(201, body), LIST_PATH, 5).unwrap(),
        Uploaded {
            name: "photo (1).jpg".into(),
            size: 5
        }
    );
    // The host stored another size than was sent.
    for length in [0, 4, 6] {
        match uploaded(response(201, body), LIST_PATH, length) {
            Err(FileError::Host(Error::Malformed(m))) => {
                assert_eq!(
                    m,
                    format!("native API files: the host stored 5 of {length} bytes")
                )
            }
            other => panic!("{length}: {other:?}"),
        }
    }
    for status in [200, 202, 204, 299] {
        match uploaded(response(status, body), LIST_PATH, 5) {
            Err(FileError::Host(Error::Status { code, .. })) => {
                assert_eq!(code, i32::from(status))
            }
            other => panic!("{status}: {other:?}"),
        }
    }
    for broken in ["", "{}", r#"{"name":"a"}"#, r#"{"name":"a","size":-1}"#] {
        assert!(
            matches!(
                uploaded(response(201, broken), LIST_PATH, 1),
                Err(FileError::Host(Error::Malformed(_)))
            ),
            "{broken}"
        );
    }
}

#[test]
fn a_download_needs_200() {
    assert_eq!(downloaded(response(200, ""), CONTENT_PATH, 7).unwrap(), 7);
    for status in [201, 204, 206, 299] {
        match downloaded(response(status, ""), CONTENT_PATH, 7) {
            Err(FileError::Host(Error::Status { code, .. })) => {
                assert_eq!(code, i32::from(status))
            }
            other => panic!("{status}: {other:?}"),
        }
    }
}

#[test]
fn a_listing_needs_200_and_its_fields() {
    let body = r#"{"folder":"C:\\Users\\u\\Downloads\\Rubylight","free_bytes":10,
            "max_file_bytes":4294967296,"files":[{"name":"a.txt","size":3,"modified_unix_ms":1}]}"#;
    let list = decode::<FileList>(response(200, body), LIST_PATH, 200).unwrap();
    assert_eq!(list.folder, r"C:\Users\u\Downloads\Rubylight");
    assert_eq!(list.max_file_bytes, 4 << 30);
    assert_eq!(
        list.files,
        [RemoteFile {
            name: "a.txt".into(),
            size: 3,
            modified_unix_ms: 1
        }]
    );
    assert!(matches!(
        decode::<FileList>(response(201, body), LIST_PATH, 200),
        Err(FileError::Host(Error::Status { code: 201, .. }))
    ));
    assert!(matches!(
        decode::<FileList>(response(200, r#"{"folder":"x"}"#), LIST_PATH, 200),
        Err(FileError::Host(Error::Malformed(_)))
    ));
}

/// A file the transfer may not write.
struct Full;

impl Write for Full {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(ErrorKind::PermissionDenied.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_cancel_wins_over_whatever_failed_after_it() {
    let failed_disk = || {
        let mut sink = Local::new(Full);
        assert!(sink.write_all(b"x").is_err());
        sink
    };
    assert!(matches!(
        failed_disk().or(Error::Cancelled),
        FileError::Host(Error::Cancelled)
    ));
    assert!(matches!(
        failed_disk().or(Error::Malformed("cut short".into())),
        FileError::Local(e) if e.kind() == ErrorKind::PermissionDenied
    ));
    assert!(matches!(
        Local::new(std::io::empty()).or(Error::Malformed("cut short".into())),
        FileError::Host(Error::Malformed(_))
    ));
    // One handle per transfer, cancelled from another thread.
    let stop = std::sync::Arc::new(TransferCancel::new());
    let other = stop.clone();
    std::thread::spawn(move || other.cancel()).join().unwrap();
    assert!(stop.is_cancelled());
}
