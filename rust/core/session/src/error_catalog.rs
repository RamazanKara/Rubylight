//! Plain-English titles and next steps for the native API's errors.
//!
//! Every error body keeps its `message`; the host adds a `title`, and a `next_step` where the
//! message does not already say what to do. A cause the caller knows (the driver refused a
//! write, a journal could not be saved) comes first, then a known failure that the technical
//! detail matches in [`log_analysis::RULES`], then what the code means. Codes a client only
//! meets through a bug of its own (`not_found`, `invalid_request`) get neither. Codes, error
//! chains and HRESULTs go in `technical_detail` only, redacted like a support bundle, so the
//! title and next step stay words a player can act on.
use crate::{log_analysis, log_redact::Redactor};
use rubylight_native_api::error::{ApiError, ErrorCode};

/// The longest technical detail an error carries; the host log has the rest.
const DETAIL_CHARS: usize = 400;

/// A headline and, when the message alone does not say it, what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub title: &'static str,
    pub next_step: Option<&'static str>,
}

const fn says(title: &'static str, next_step: Option<&'static str>) -> Option<Explanation> {
    Some(Explanation { title, next_step })
}

/// What `code` means for the person holding the device, or `None` when a title would only
/// repeat a client's bug back to it.
pub fn for_code(code: ErrorCode) -> Option<Explanation> {
    use ErrorCode::*;
    match code {
        NotPaired => says(
            "This device is not paired with the PC",
            Some("Pair this device with the PC again."),
        ),
        PermissionRequired => says("This device needs a permission", None),
        HostControlRequired => says("Host control is off for this device", None),
        Forbidden => says("This device is not allowed to do that", None),
        NoSession => says("No session is running", None),
        NotSessionOwner => says(
            "Another device owns this session",
            Some("Use the device that started it, or move the stream here with Continue here."),
        ),
        SessionBusy => says("The PC is starting or stopping a stream", None),
        DriverRejected => says(
            "The graphics driver refused the change",
            Some(
                "The PC put the earlier values back. Choose another value, or update the graphics driver on the PC.",
            ),
        ),
        TemporarilyUnavailable => says("The PC can't do this right now", None),
        Internal => says(
            "Something went wrong on the PC",
            Some(
                "Try again. If it keeps failing, open Help & diagnostics in the Rubylight console on the PC.",
            ),
        ),
        FileTransferDisabled => says("File transfer is off on the PC", None),
        InvalidName => says(
            "The PC refuses this file name",
            Some("Rename the file, then try again."),
        ),
        IncompleteUpload => says(
            "The upload did not finish",
            Some("Check the connection, then send the file again."),
        ),
        TooLarge => says(
            "The file is too large for the PC",
            Some("Send a smaller file."),
        ),
        InsufficientStorage => says(
            "The PC's drive is full",
            Some("Free up space on the PC, then send the file again."),
        ),
        NoConsoleUser => says("Nobody is signed in on the PC", None),
        TransferBusy => says("A file transfer is already running", None),
        UsbDisabled => says("USB forwarding is off", None),
        UsbLimit => says(
            "The PC forwards as many USB devices as it allows",
            Some("Detach a USB device you no longer use, then try again."),
        ),
        FoldersDisabled => says("No folders are shared", None),
        FolderUnreadable => says("Windows blocks this folder", None),
        CoopRefused => says("This co-op game can't be joined", None),
        HostInUse => says(
            "The PC is in use",
            Some("Wait until it is free, or end the other stream on its device, then try again."),
        ),
        TokenExpired => says("The confirmation expired", None),
        UpdateUnavailable => says("The update can't be installed", None),
        WrongPin => says("The PIN does not match", None),
        PairingUnconfirmed => says("The new device did not finish pairing", None),
        PairingEnded => says("Too many wrong PINs", None),
        PairingCooldown => says("A wrong PIN just came from this device", None),
        NotFound | InvalidRequest | InvalidValue | NotWritable | LengthRequired | NotStreaming
        | Unknown => None,
    }
}

/// The known failure a technical text names, as the console's Help & diagnostics words it.
pub fn for_detail(detail: &str) -> Option<Explanation> {
    log_analysis::rule_for(detail).map(|rule| Explanation {
        title: rule.title,
        next_step: Some(rule.fix),
    })
}

/// `text` as a technical detail: user folders, addresses and secrets redacted as in a support
/// bundle, and cut at [`DETAIL_CHARS`].
pub fn detail(text: &str) -> String {
    let text = Redactor::new().line(text.trim());
    match text.char_indices().nth(DETAIL_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

/// `error` with a title and next step when it has no title of its own: from the known failure
/// its technical detail matches, else from its code.
pub fn explain(mut error: ApiError) -> ApiError {
    if error.title.is_none() {
        let known = error.technical_detail.as_deref().and_then(for_detail);
        if let Some(explanation) = known.or_else(|| for_code(error.code)) {
            error.title = Some(explanation.title.to_owned());
            error.next_step = explanation.next_step.map(str::to_owned);
        }
    }
    error
}

#[cfg(test)]
mod tests;
