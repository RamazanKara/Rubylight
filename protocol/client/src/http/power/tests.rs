use super::*;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The document Vibepollo's Boost writer prints, and Rubylight's copy of it.
fn document(status: u16, message: &str, flags: [u8; 3], token: &str) -> String {
    let token = if token.is_empty() {
        "<SleepToken/>".to_owned()
    } else {
        format!("<SleepToken>{token}</SleepToken>")
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"{status}\" \
             status_message=\"{message}\"><PowerActionVersion>1</PowerActionVersion>\
             <SleepSupported>{}</SleepSupported><SleepAllowed>{}</SleepAllowed>\
             <SleepAvailable>{}</SleepAvailable>\
             <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy>{token}\
             <SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
             <SleepAccepted>{}</SleepAccepted></root>",
        flags[0],
        flags[1],
        flags[2],
        u8::from(status == 202)
    )
}

fn response(status: u16, body: &str) -> transport::Response {
    transport::Response {
        status,
        reason: "Reason".into(),
        content_type: Some("application/xml".into()),
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn an_offer_carries_the_token() {
    let reply = decode(
        response(200, &document(200, "", [1, 1, 1], TOKEN)),
        CAPABILITIES_PATH,
        200,
    )
    .unwrap();
    assert_eq!(
        reply,
        PowerReply {
            status: 200,
            message: String::new(),
            version: 1,
            sleep_supported: true,
            sleep_allowed: true,
            sleep_available: true,
            active_session_policy: "idle-only".into(),
            token: Some(TOKEN.into()),
            token_lifetime_seconds: 30,
            accepted: false,
        }
    );
    // Busy, not allowed or unsupported: still a 200, with no token.
    for flags in [[1, 1, 0], [1, 0, 0], [0, 0, 0]] {
        let body =
            document(200, "", flags, "").replace("<SleepToken/>", "<SleepToken></SleepToken>");
        let reply = decode(response(200, &body), CAPABILITIES_PATH, 200).unwrap();
        assert_eq!((reply.token, reply.sleep_available), (None, false));
    }
}

#[test]
fn an_accepted_sleep_is_the_only_post_success() {
    let message = "Sleep request accepted; native completion is not confirmed";
    let reply = decode(
        response(202, &document(202, message, [1, 1, 0], "")),
        SLEEP_PATH,
        202,
    )
    .unwrap();
    assert!(reply.accepted && reply.token.is_none());
    assert_eq!(reply.message, message);
    // A 200 to the POST is not an acceptance.
    assert!(matches!(
        decode(
            response(200, &document(200, "", [1, 1, 1], TOKEN)),
            SLEEP_PATH,
            202
        ),
        Err(PowerError::Host(Error::Status { code: 200, .. }))
    ));
}

#[test]
fn every_documented_refusal_keeps_its_reply() {
    for (status, refusal, message) in [
        (
            400,
            Refusal::BadRequest,
            "Expected one sleep token and an empty body; browser origins are not accepted",
        ),
        (
            401,
            Refusal::Unauthorized,
            "A paired TLS client identity is required",
        ),
        (
            403,
            Refusal::Forbidden,
            "Sleep control is disabled for this paired client",
        ),
        (
            405,
            Refusal::MethodNotAllowed,
            "Use POST for a sleep request",
        ),
        (
            409,
            Refusal::Conflict,
            "End all applications and sessions before requesting sleep",
        ),
        (
            409,
            Refusal::Conflict,
            "Sleep token expired or already used; confirm the action again",
        ),
        (
            501,
            Refusal::NotImplemented,
            "Native sleep control is unavailable on this host",
        ),
        (
            503,
            Refusal::Unavailable,
            "Could not create a sleep request token",
        ),
    ] {
        let body = document(status, message, [1, 0, 0], "");
        for (path, success) in [(SLEEP_PATH, 202), (CAPABILITIES_PATH, 200)] {
            match decode(response(status, &body), path, success) {
                Err(PowerError::Refused { refusal: r, reply }) => {
                    assert_eq!(
                        (r, reply.status, reply.message.as_str()),
                        (refusal, status, message)
                    );
                    assert!(!reply.accepted);
                }
                other => panic!("{status}: {other:?}"),
            }
        }
    }
}

#[test]
fn unknown_answers_are_errors_not_successes() {
    let status = |result: Result<PowerReply, PowerError>| match result {
        Err(PowerError::Host(Error::Status { code, .. })) => code,
        other => panic!("{other:?}"),
    };
    // An undocumented status with a power document, and refusals without one.
    assert_eq!(
        status(decode(
            response(500, &document(500, "x", [0, 0, 0], "")),
            SLEEP_PATH,
            202
        )),
        500
    );
    assert_eq!(
        status(decode(
            response(401, "<root status_code=\"401\"/>"),
            SLEEP_PATH,
            202
        )),
        401
    );
    // Vibepollo's failed certificate check: HTTP 200, the refusal in the document.
    assert_eq!(
        status(decode(
            response(200, "<root status_code=\"401\"/>"),
            CAPABILITIES_PATH,
            200
        )),
        401
    );
    assert_eq!(
        status(decode(response(503, "busy"), CAPABILITIES_PATH, 200)),
        503
    );
    assert!(matches!(
        decode(response(404, "<html/>"), CAPABILITIES_PATH, 200),
        Err(PowerError::Host(Error::NotFound(p))) if p == "/power-actions/v1"
    ));
    let malformed = |status: u16, body: &str, success: u16| {
        let result = decode(response(status, body), SLEEP_PATH, success);
        assert!(
            matches!(result, Err(PowerError::Host(Error::Malformed(_)))),
            "{body}: {result:?}"
        );
    };
    // A success without a power document, or with one that does not hold.
    malformed(202, "", 202);
    malformed(202, "<root status_code=\"202\"/>", 202);
    malformed(202, &document(200, "", [1, 1, 1], ""), 202);
    malformed(
        200,
        &document(200, "", [1, 1, 1], "").replace("<SleepAccepted>0", "<SleepAccepted>1"),
        200,
    );
    malformed(
        202,
        &document(202, "", [1, 1, 0], "").replace("<SleepAccepted>1", "<SleepAccepted>0"),
        202,
    );
    malformed(200, &document(200, "", [2, 1, 1], ""), 200);
    malformed(200, &document(200, "", [1, 1, 1], "ABC"), 200);
    malformed(
        200,
        &document(200, "", [1, 1, 1], &TOKEN.to_uppercase()),
        200,
    );
    malformed(
        200,
        &document(200, "", [1, 1, 1], "")
            .replace("<PowerActionVersion>1", "<PowerActionVersion>0"),
        200,
    );
    malformed(
        200,
        &document(200, "", [1, 1, 1], "").replace(
            "<SleepTokenLifetimeSeconds>30",
            "<SleepTokenLifetimeSeconds>",
        ),
        200,
    );
    // A refusal whose document is broken is malformed too, not a refusal.
    malformed(409, &document(409, "x", [7, 0, 0], ""), 202);
}

#[test]
fn a_bad_token_is_never_sent() {
    // Port 1 on loopback refuses connections: an Io error would mean a request went out.
    let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
    let client = HostClient::new("127.0.0.1", 1, identity)
        .unwrap()
        .with_https_port(1);
    for token in ["", "abc", &TOKEN.to_uppercase(), &format!("{TOKEN}&uuid=x")] {
        assert!(
            matches!(
                client.sleep(token),
                Err(PowerError::Host(Error::InvalidArgument(_)))
            ),
            "{token}"
        );
    }
}
