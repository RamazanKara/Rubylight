//! Shared folders (`docs/native-api.md`, "Shared folders"): a paired device
//! browses the folders the PC shares with it, downloads from them and, with
//! a `write` grant, uploads to them.
//!
//! The host's `shared_folders` setting names the folders and a device's
//! grants (`Client::folder_access`) say which it sees and whether it may
//! write: never a `perm` bit or Full control. Both are read on each request
//! and again every 250 ms while a transfer runs, so dropping a folder or a
//! grant, or moving the folder, ends a running transfer. Files are opened as
//! the signed-in Windows user, confined to the folder's root
//! (`butterpollo_windows::shared_folder`), and move with file transfer's
//! limits: its size limit, free-space reserve, slots and stream rate. The
//! File transfer setting itself isn't needed.
use super::{
    Failure, Reply, caller,
    files::{self, Recheck},
    json,
};
use crate::{state::Shared, tls::Connection};
use axum::{
    Extension,
    body::{Body, HttpBody},
    extract::{Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::{
    config::SharedFolderConfig,
    file_transfer,
    native_api::{
        error::ErrorCode,
        folders::{EntryKind, FolderAccess, FolderEntries, FolderEntry, FolderList, SharedFolder},
    },
    state::Client,
};
use butterpollo_windows::{shared_folder::SharedFolder as Root, user_folder::UserFolderError};
use std::{io, sync::PoisonError};

/// The folders the host shares; none is `folders_disabled`.
fn shared(h: &Shared) -> Result<Vec<SharedFolderConfig>, Failure> {
    let folders = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .shared_folders();
    if folders.is_empty() {
        return Err(Failure::new(
            ErrorCode::FoldersDisabled,
            "This PC shares no folders. Add one in the Rubylight console.",
        ));
    }
    Ok(folders)
}

/// The caller and the folder `id`, if the device has a grant for it, a
/// `write` one when `write` is set.
fn granted(
    h: &Shared,
    c: &Connection,
    id: &str,
    write: bool,
) -> Result<(Client, SharedFolderConfig), Failure> {
    let client = caller(h, c)?;
    let folders = shared(h)?;
    let Some(access) = client.folder_access(id) else {
        return Err(Failure::new(
            ErrorCode::Forbidden,
            "This folder isn't shared with this device. Share it in the Rubylight console.",
        ));
    };
    if write && access != FolderAccess::Write {
        return Err(Failure::new(
            ErrorCode::Forbidden,
            "This device may only read this folder. Let it write in the Rubylight console.",
        ));
    }
    let folder = folders
        .into_iter()
        .find(|folder| folder.id == id)
        .ok_or_else(gone)?;
    Ok((client, folder))
}
fn gone() -> Failure {
    Failure::new(ErrorCode::NotFound, "The PC no longer shares this folder.")
}
/// [`granted`] again every 250 ms while a transfer runs, for the same
/// folder at the same place.
fn recheck(h: &Shared, c: &Connection, folder: &SharedFolderConfig, write: bool) -> Recheck {
    let (h, c) = (h.clone(), c.clone());
    let (id, path) = (folder.id.clone(), folder.path.clone());
    Recheck::with(move || {
        let (_, folder) = granted(&h, &c, &id, write)?;
        if folder.path != path {
            return Err(gone());
        }
        Ok(())
    })
}

/// The folder id from the route. One that isn't UTF-8 can't have a grant.
fn id_of(id: Result<Path<String>, PathRejection>) -> Result<String, Failure> {
    id.map(|Path(id)| id).map_err(|_| {
        Failure::new(
            ErrorCode::Forbidden,
            "This folder isn't shared with this device.",
        )
    })
}

/// The `path` query argument as names below the folder, percent-decoded as
/// `name` is for `/files`; missing or empty for the folder itself.
fn names_of(query: Option<&str>) -> Result<Vec<String>, Failure> {
    let invalid = |message: &str| Failure::new(ErrorCode::InvalidName, message);
    let mut found = None;
    for pair in query.unwrap_or_default().split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "path" {
            continue;
        }
        if found.is_some() {
            return Err(invalid("Send one path."));
        }
        found = Some(
            files::decode(value)
                .ok_or_else(|| invalid("The path is not percent-encoded UTF-8."))?,
        );
    }
    let path = found.unwrap_or_default();
    let names = file_transfer::folder_path(&path)
        .map_err(|error| failure("check the path", error.into()))?;
    Ok(names.into_iter().map(str::to_owned).collect())
}
fn refs(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

/// The folder's root, as the signed-in user. A root that can't be opened is
/// the administrator's to fix, so it is logged.
fn open_root(folder: &SharedFolderConfig) -> Result<Root, UserFolderError> {
    open_as(&folder.path).inspect_err(|error| {
        if matches!(error, UserFolderError::Io(_)) {
            tracing::warn!(
                "Shared folders could not open {} at {}: {error}",
                folder.name,
                folder.path.display()
            );
        }
    })
}
#[cfg(not(test))]
fn open_as(path: &std::path::Path) -> Result<Root, UserFolderError> {
    Root::open(path)
}
/// Tests open their folders as the developer, as file transfer's do.
#[cfg(test)]
fn open_as(path: &std::path::Path) -> Result<Root, UserFolderError> {
    Root::unimpersonated(path)
}

/// A folder failure as the error the spec gives it: what isn't there, or
/// isn't there below the folder's root, is `not_found`.
fn failure(what: &str, error: UserFolderError) -> Failure {
    match error {
        UserFolderError::OutsideFolder | UserFolderError::NotPlainFile => not_found(),
        UserFolderError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            not_found()
        }
        error => files::failure(what, error),
    }
}
fn not_found() -> Failure {
    Failure::new(
        ErrorCode::NotFound,
        "There is no such file or folder in this shared folder.",
    )
}
/// Runs `work` on the folder's root on a blocking thread.
async fn in_folder<T: Send + 'static>(
    what: &'static str,
    folder: &SharedFolderConfig,
    work: impl FnOnce(&Root) -> Result<T, UserFolderError> + Send + 'static,
) -> Result<T, Failure> {
    let folder = folder.clone();
    tokio::task::spawn_blocking(move || work(&open_root(&folder)?))
        .await
        .map_err(|error| Failure::internal(what, error))?
        .map_err(|error| failure(what, error))
}

/// `GET /folders`: the folders shared with this device, in the host's
/// order. A device with a grant for none of them is `forbidden`.
pub(super) async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let folders: Vec<SharedFolder> = shared(&h)?
        .into_iter()
        .filter_map(|folder| {
            let access = client.folder_access(&folder.id)?;
            Some(SharedFolder {
                id: folder.id,
                name: folder.name,
                access,
            })
        })
        .collect();
    if folders.is_empty() {
        return Err(Failure::new(
            ErrorCode::Forbidden,
            "This PC shares no folder with this device. Share one in the Rubylight console.",
        ));
    }
    Ok(json(StatusCode::OK, &FolderList { folders }))
}

/// `GET /folders/{id}/entries?path=`: the folders and plain files in a
/// directory of the folder, folders first.
pub(super) async fn entries(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    id: Result<Path<String>, PathRejection>,
    request: Request,
) -> Reply {
    let (_, folder) = granted(&h, &c, &id_of(id)?, false)?;
    let names = names_of(request.uri().query())?;
    let path = names.join("/");
    let listed = in_folder("list the shared folder", &folder, move |root| {
        root.entries(&refs(&names))
    })
    .await?;
    let entries = listed
        .into_iter()
        .map(|entry| FolderEntry {
            name: entry.name,
            kind: if entry.dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            size: entry.size,
            modified_unix_ms: entry.modified_unix_ms,
        })
        .collect();
    Ok(json(StatusCode::OK, &FolderEntries { path, entries }))
}

/// `GET /folders/{id}/content?path=`: the file with its exact length, as
/// `GET /files/content` sends it.
pub(super) async fn content(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    id: Result<Path<String>, PathRejection>,
    request: Request,
) -> Reply {
    let (client, folder) = granted(&h, &c, &id_of(id)?, false)?;
    let names = names_of(request.uri().query())?;
    let slot = files::begin_download(&h, &client.uuid)?;
    let (file, length) = in_folder("open the file", &folder, move |root| {
        root.open_file(&refs(&names))
    })
    .await?;
    let recheck = recheck(&h, &c, &folder, false);
    Ok(files::send_file(h, slot, file, length, recheck))
}

/// `PUT /folders/{id}/content?path=`: an upload into a directory of the
/// folder, answered and refused as `POST /files` is.
pub(super) async fn upload(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    id: Result<Path<String>, PathRejection>,
    request: Request,
) -> Response {
    let (parts, mut body) = request.into_parts();
    if let Err(refusal) = caller(&h, &c) {
        // Reading on would let anyone send the host data.
        refused_upload(&c, &refusal);
        return files::closing(refusal.into_response());
    }
    let received = receive(&h, &c, id, parts.uri.query(), &parts.headers, &mut body).await;
    let mut response = match received {
        Ok(response) => response,
        Err(failure) => {
            refused_upload(&c, &failure);
            failure.into_response()
        }
    };
    if !body.is_end_stream() {
        tokio::spawn(files::drain(h, body));
        response = files::closing(response);
    }
    response
}
fn refused_upload(c: &Connection, failure: &Failure) {
    tracing::warn!(
        "Shared folders refused an upload from {}: {}",
        c.peer.ip(),
        failure.message
    );
}
async fn receive(
    h: &Shared,
    c: &Connection,
    id: Result<Path<String>, PathRejection>,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &mut Body,
) -> Reply {
    let (client, folder) = granted(h, c, &id_of(id)?, true)?;
    let mut names = names_of(query)?;
    let Some(name) = names.pop() else {
        return Err(Failure::new(
            ErrorCode::InvalidName,
            "Send the file's path in the folder as ?path=.",
        ));
    };
    file_transfer::check_name(&name).map_err(|error| failure("check the name", error.into()))?;
    let recheck = recheck(h, c, &folder, true);
    let target = folder.clone();
    let uploaded = files::accept(
        h,
        &client.uuid,
        headers,
        body,
        recheck,
        failure,
        move |slot, length| {
            let root = open_root(&target)?;
            let dir = root.upload_folder(&refs(&names))?;
            slot.hold_room(&dir, length)?;
            root.create_part(&dir, &name)
        },
    )
    .await?;
    tracing::info!(
        "Shared folders received {} ({} bytes) in {} from {}",
        uploaded.name,
        uploaded.size,
        folder.name,
        client.name
    );
    Ok(json(StatusCode::CREATED, &uploaded))
}
