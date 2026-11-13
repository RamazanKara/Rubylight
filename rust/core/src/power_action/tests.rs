use super::*;

fn phone(allow_sleep: bool) -> Authority {
    Authority {
        uuid: "phone".into(),
        allow_sleep,
        allow_restart_shutdown: false,
    }
}
fn bytes(b: u8) -> Option<[u8; 32]> {
    Some([b; 32])
}
fn idle() -> Option<()> {
    Some(())
}
fn busy() -> Option<()> {
    None
}
fn post(token: &str) -> Request {
    Request::parse(true, Some(&format!("token={token}")), 0, false)
}
fn token(tokens: &mut Tokens, caller: &Authority, now: Instant) -> String {
    let r = tokens.capabilities(Some(caller), true, idle, now, || bytes(0xab));
    assert!(r.available, "{r:?}");
    r.token
}
fn sleep(
    tokens: &mut Tokens,
    request: &Request,
    caller: Option<&Authority>,
    now: Instant,
) -> Response {
    tokens.sleep(request, caller, || true, idle, now).response
}

#[test]
fn documents_match_vibepollo_byte_for_byte() {
    let mut tokens = Tokens::default();
    let now = Instant::now();
    let r = tokens.capabilities(Some(&phone(true)), true, idle, now, || bytes(0x0f));
    assert_eq!(
        r.xml(),
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"200\" status_message=\"\">\
                 <PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>\
                 <SleepAllowed>1</SleepAllowed><SleepAvailable>1</SleepAvailable>\
                 <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy>\
                 <SleepToken>{}</SleepToken><SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
                 <SleepAccepted>0</SleepAccepted></root>",
            "0f".repeat(32)
        )
    );
    let accepted = sleep(&mut tokens, &post(&r.token), Some(&phone(true)), now);
    assert_eq!(
        accepted.xml(),
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"202\" \
             status_message=\"Sleep request accepted; native completion is not confirmed\">\
             <PowerActionVersion>1</PowerActionVersion><SleepSupported>0</SleepSupported>\
             <SleepAllowed>0</SleepAllowed><SleepAvailable>0</SleepAvailable>\
             <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy><SleepToken/>\
             <SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
             <SleepAccepted>1</SleepAccepted></root>"
    );
}

#[test]
fn capabilities_hand_a_token_only_to_an_allowed_client_of_an_idle_host() {
    let mut tokens = Tokens::default();
    let now = Instant::now();
    let none = tokens.capabilities(None, true, idle, now, || bytes(1));
    assert_eq!((none.status, none.message), (401, NO_IDENTITY));
    let off = tokens.capabilities(Some(&phone(false)), true, idle, now, || bytes(1));
    assert_eq!(
        (off.status, off.supported, off.allowed, off.available),
        (200, true, false, false)
    );
    assert!(off.token.is_empty());
    let unsupported = tokens.capabilities(Some(&phone(true)), false, idle, now, || bytes(1));
    assert_eq!(
        (
            unsupported.supported,
            unsupported.allowed,
            unsupported.available
        ),
        (false, true, false)
    );
    let busy = tokens.capabilities(Some(&phone(true)), true, busy, now, || bytes(1));
    assert_eq!(
        (busy.status, busy.supported, busy.allowed, busy.available),
        (200, true, true, false)
    );
    assert!(busy.token.is_empty());
    let failed = tokens.capabilities(Some(&phone(true)), true, idle, now, || None);
    assert_eq!(
        (failed.status, failed.message, failed.available),
        (503, "Could not create a sleep request token", false)
    );
    assert!(tokens.tokens.is_empty());
    let ok = tokens.capabilities(Some(&phone(true)), true, idle, now, || bytes(0xab));
    assert_eq!((ok.status, ok.available, ok.token.len()), (200, true, 64));
}

#[test]
fn every_sleep_refusal_in_vibepollos_order() {
    let mut tokens = Tokens::default();
    let now = Instant::now();
    let token = token(&mut tokens, &phone(true), now);
    let get = Request {
        post: false,
        ..post(&token)
    };
    let check = |r: Response, status: u16, message: &str| {
        assert_eq!((r.status, r.message), (status, message));
        assert!(!r.xml().contains("<SleepAccepted>1"));
    };
    let mut t = Tokens::default();
    check(
        sleep(&mut t, &get, None, now),
        405,
        "Use POST for a sleep request",
    );
    let shape = "Expected one sleep token and an empty body; browser origins are not accepted";
    check(sleep(&mut t, &post("ABCD"), None, now), 400, shape);
    check(
        sleep(&mut t, &post(&token.to_uppercase()), None, now),
        400,
        shape,
    );
    check(sleep(&mut t, &post(&token), None, now), 401, NO_IDENTITY);
    check(
        sleep(&mut t, &post(&token), Some(&phone(false)), now),
        403,
        "Sleep control is disabled for this paired client",
    );
    check(
        tokens
            .sleep(&post(&token), Some(&phone(true)), || false, idle, now)
            .response,
        501,
        "Native sleep control is unavailable on this host",
    );
    let decision = tokens.sleep(&post(&token), Some(&phone(true)), || true, busy, now);
    assert!(decision.gate.is_none());
    check(
        decision.response,
        409,
        "End all applications and sessions before requesting sleep",
    );
    // None of these spent the token.
    let accepted = tokens.sleep(&post(&token), Some(&phone(true)), || true, idle, now);
    assert_eq!(accepted.response.status, 202);
    assert!(accepted.gate.is_some());
}

#[test]
fn tokens_expire_are_single_use_and_replaced() {
    let used = "Sleep token expired or already used; confirm the action again";
    let now = Instant::now();
    let caller = phone(true);
    // Spent once.
    let mut tokens = Tokens::default();
    let first = token(&mut tokens, &caller, now);
    assert_eq!(
        sleep(&mut tokens, &post(&first), Some(&caller), now).status,
        202
    );
    let replay = tokens.sleep(&post(&first), Some(&caller), || true, idle, now);
    assert_eq!(
        (replay.response.status, replay.response.message),
        (409, used)
    );
    assert!(replay.gate.is_none());
    // Expired at 30 s, still good just before.
    let t = token(&mut tokens, &caller, now);
    let late = sleep(&mut tokens, &post(&t), Some(&caller), now + TOKEN_LIFETIME);
    assert_eq!((late.status, late.message), (409, used));
    let t = token(&mut tokens, &caller, now);
    let just = now + TOKEN_LIFETIME - Duration::from_millis(1);
    assert_eq!(
        sleep(&mut tokens, &post(&t), Some(&caller), just).status,
        202
    );
    // A new GET replaces the client's earlier token.
    let old = tokens
        .capabilities(Some(&caller), true, idle, now, || bytes(1))
        .token;
    let new = tokens
        .capabilities(Some(&caller), true, idle, now, || bytes(2))
        .token;
    assert_eq!(
        sleep(&mut tokens, &post(&old), Some(&caller), now).message,
        used
    );
    assert_eq!(
        sleep(&mut tokens, &post(&new), Some(&caller), now).status,
        202
    );
    // Another client's token is not this client's.
    let other = Authority {
        uuid: "tablet".into(),
        allow_sleep: true,
        allow_restart_shutdown: false,
    };
    let theirs = token(&mut tokens, &other, now);
    assert_eq!(
        sleep(&mut tokens, &post(&theirs), Some(&caller), now).message,
        used
    );
    assert_eq!(
        sleep(&mut tokens, &post(&theirs), Some(&other), now).status,
        202
    );
}

#[test]
fn at_most_256_live_tokens() {
    let now = Instant::now();
    let mut tokens = Tokens::default();
    for i in 0..MAX_TOKENS {
        let caller = Authority {
            uuid: format!("c{i}"),
            allow_sleep: true,
            allow_restart_shutdown: false,
        };
        token(&mut tokens, &caller, now);
    }
    let late = Authority {
        uuid: "late".into(),
        allow_sleep: true,
        allow_restart_shutdown: false,
    };
    let full = tokens.capabilities(Some(&late), true, idle, now, || bytes(1));
    assert_eq!(
        (full.status, full.available, full.token.as_str()),
        (200, false, "")
    );
    // A client with a live token may still replace it.
    let first = Authority {
        uuid: "c0".into(),
        allow_sleep: true,
        allow_restart_shutdown: false,
    };
    assert!(
        tokens
            .capabilities(Some(&first), true, idle, now, || bytes(2))
            .available
    );
    // Expired tokens free their places.
    let later = now + TOKEN_LIFETIME;
    assert!(
        tokens
            .capabilities(Some(&late), true, idle, later, || bytes(3))
            .available
    );
    assert_eq!(tokens.tokens.len(), 1);
}

#[test]
fn only_a_bare_token_query_without_body_or_origin_has_the_shape() {
    let t = "ab".repeat(32);
    assert!(Request::parse(true, Some(&format!("token={t}")), 0, false).valid_shape);
    // Names compare without case, values are percent-decoded.
    let r = Request::parse(true, Some(&format!("TOKEN=%61{}", &t[1..])), 0, false);
    assert!(r.valid_shape);
    assert_eq!(r.token, t);
    assert!(Request::parse(true, Some(&format!("token={t}&")), 0, false).valid_shape);
    for (query, body, origin) in [
        (format!("token={t}"), 1, false),
        (format!("token={t}"), 0, true),
        (format!("token={t}&x=1"), 0, false),
        (format!("token={t}&token={t}"), 0, false),
        (format!("other={t}"), 0, false),
        (String::new(), 0, false),
    ] {
        let request = Request::parse(true, Some(&query), body, origin);
        assert!(!request.valid_shape, "{query} {body} {origin}");
        let mut tokens = Tokens::default();
        let r = tokens
            .sleep(&request, Some(&phone(true)), || true, idle, Instant::now())
            .response;
        assert_eq!(r.status, 400);
    }
    assert!(!Request::parse(true, None, 0, false).valid_shape);
}

#[test]
fn sleep_pc_needs_s1_to_s3_and_says_why_not() {
    let states = |s3, modern_standby, hibernate| SleepStates {
        s3,
        modern_standby,
        hibernate,
        ..Default::default()
    };
    let desktop = states(true, false, true);
    assert_eq!(desktop.unavailable(), None);
    assert_eq!(desktop.describe(), "S3 available, hibernate on");
    // S3 beside Modern Standby still sleeps.
    assert_eq!(states(true, true, false).unavailable(), None);
    let s1 = SleepStates {
        s1: true,
        ..Default::default()
    };
    assert_eq!(s1.unavailable(), None);
    let laptop = states(false, true, true);
    assert_eq!(
        laptop.unavailable(),
        Some("this PC only has Modern Standby, so Windows would hibernate it")
    );
    assert_eq!(laptop.describe(), "Modern Standby available, hibernate on");
    let no_hibernate = states(false, true, false);
    assert_eq!(
        no_hibernate.unavailable(),
        Some("this PC only has Modern Standby, which Windows offers no call to enter")
    );
    assert_eq!(
        no_hibernate.describe(),
        "Modern Standby available, hibernate off"
    );
    let none = states(false, false, true);
    assert_eq!(none.unavailable(), Some("this PC has no sleep state"));
    assert_eq!(none.describe(), "no sleep state, hibernate on");
    let all = SleepStates {
        s1: true,
        s2: true,
        ..states(true, true, true)
    };
    assert_eq!(
        all.describe(),
        "S1, S2, S3, Modern Standby available, hibernate on"
    );
}

fn granted(allow_restart_shutdown: bool) -> Authority {
    Authority {
        uuid: "phone".into(),
        allow_sleep: false,
        allow_restart_shutdown,
    }
}
fn free() -> Result<(), Busy> {
    Ok(())
}

#[test]
fn restart_and_shutdown_need_their_own_grant_not_sleeps() {
    let mut tokens = Tokens::default();
    let now = Instant::now();
    let sleeper = phone(true);
    let offer = tokens.offer(&sleeper, free, now, || bytes(1));
    assert_eq!(
        offer,
        Offer {
            reason: Some(RESTART_SHUTDOWN_OFF.into()),
            token: None
        }
    );
    // Sleep PC's token does not restart the PC.
    let sleep_token = token(&mut tokens, &sleeper, now);
    assert_eq!(
        tokens.power(&sleeper, &sleep_token, free, now),
        Err(Refused::NotAllowed)
    );
    // Nor does the grant open Sleep PC.
    let restarter = granted(true);
    let r = tokens.capabilities(Some(&restarter), true, idle, now, || bytes(2));
    assert!(!r.allowed && r.token.is_empty());
}

#[test]
fn a_busy_pc_is_refused_with_what_keeps_it_busy_and_the_token_survives() {
    let mut tokens = Tokens::default();
    let now = Instant::now();
    let caller = granted(true);
    let streaming = || -> Result<(), Busy> { Err(Busy::Streaming(vec!["Living room TV".into()])) };
    let offer = tokens.offer(&caller, streaming, now, || bytes(1));
    assert_eq!(
        offer.reason.as_deref(),
        Some("Living room TV is streaming from this PC.")
    );
    assert_eq!(offer.token, None);
    let t = tokens.offer(&caller, free, now, || bytes(3)).token.unwrap();
    assert_eq!(t, "03".repeat(32));
    assert_eq!(
        tokens.power(&caller, &t, streaming, now),
        Err(Refused::Busy(Busy::Streaming(vec![
            "Living room TV".into()
        ])))
    );
    assert_eq!(tokens.power(&caller, &t, free, now), Ok(()));
}

#[test]
fn power_tokens_are_single_use_expire_and_belong_to_one_device() {
    let now = Instant::now();
    let caller = granted(true);
    let mut tokens = Tokens::default();
    let t = tokens.offer(&caller, free, now, || bytes(1)).token.unwrap();
    assert_eq!(tokens.power(&caller, &t, free, now), Ok(()));
    assert_eq!(tokens.power(&caller, &t, free, now), Err(Refused::Token));
    let t = tokens.offer(&caller, free, now, || bytes(1)).token.unwrap();
    assert_eq!(
        tokens.power(&caller, &t, free, now + TOKEN_LIFETIME),
        Err(Refused::Token)
    );
    let t = tokens
        .offer(&caller, free, now, || bytes(0xab))
        .token
        .unwrap();
    assert_eq!(
        tokens.power(&caller, &t.to_uppercase(), free, now),
        Err(Refused::Token)
    );
    let other = Authority {
        uuid: "tablet".into(),
        ..granted(true)
    };
    assert_eq!(tokens.power(&other, &t, free, now), Err(Refused::Token));
    assert_eq!(tokens.power(&caller, &t, free, now), Ok(()));
    // No random bytes, no token.
    let failed = tokens.offer(&caller, free, now, || None);
    assert_eq!(failed.reason.as_deref(), Some(NO_TOKEN));
}

#[test]
fn every_busy_state_says_what_it_is() {
    let messages: Vec<_> = [
        Busy::Streaming(vec!["TV".into()]),
        Busy::Streaming(vec!["TV".into(), "Phone".into(), "Deck".into()]),
        Busy::Starting(vec!["Phone".into()]),
        Busy::App("Steam".into()),
        Busy::Transition,
        Busy::Ending,
        Busy::Guest,
        Busy::Controller,
    ]
    .iter()
    .map(Busy::message)
    .collect();
    assert_eq!(
        messages,
        [
            "TV is streaming from this PC.",
            "TV, Phone and Deck are streaming from this PC.",
            "Phone is starting a stream.",
            "Steam is still running on this PC.",
            "A stream is starting or stopping.",
            "A stream is still ending.",
            "A browser guest is connected.",
            "A controller session is in use.",
        ]
    );
    assert_eq!(
        (Action::Restart.name(), Action::Shutdown.event()),
        ("restart", "power.shutdown")
    );
}

#[test]
fn random_tokens_are_64_lowercase_hex() {
    let mut tokens = Tokens::default();
    let r = tokens.capabilities(Some(&phone(true)), true, idle, Instant::now(), os_random);
    assert!(valid_token(&r.token), "{}", r.token);
}
