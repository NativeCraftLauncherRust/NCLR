//! Drives the sign-in state machine the way an engine would: from a frame loop,
//! with the caller owning the clock.
//!
//! The point is that `SignIn` never sleeps and never blocks. An engine has a
//! frame loop, not a thread to spare for a prompt, so it can call `tick` only
//! when the interval elapses and do something else in between.
//!
//! Run with: cargo run --example signin_loop --offline

use hzr_download::auth::{self, SignIn, Step};
use hzr_download::Transport;
use std::cell::RefCell;

/// A stand-in for Microsoft, with a clock the test controls.
struct Fake {
    polls: RefCell<usize>,
    /// How many polls answer "still waiting" before the player finishes typing.
    pending_until: usize,
    requests: RefCell<Vec<String>>,
}

impl Fake {
    fn new(pending_until: usize) -> Fake {
        Fake {
            polls: RefCell::new(0),
            pending_until,
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for Fake {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        Err(format!("{url}: unexpected GET"))
    }

    /// The entitlement check reads with a bearer token, not a POST.
    fn fetch_authorized(&self, url: &str, token: &str) -> Result<Vec<u8>, String> {
        self.requests.borrow_mut().push(format!("{url} (token {token})"));
        if url == hzr_download::ENTITLEMENTS {
            Ok(br#"{"items":[{"name":"product_minecraft"}]}"#.to_vec())
        } else {
            Err(format!("{url}: not in the fixture"))
        }
    }

    fn post(&self, url: &str, _kind: &str, _body: &str) -> Result<Vec<u8>, String> {
        self.requests.borrow_mut().push(url.to_string());
        match url {
            auth::DEVICE_CODE => Ok(br#"{"device_code":"dc","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/link","interval":5,"expires_in":900}"#.to_vec()),
            auth::TOKEN => {
                let mut polls = self.polls.borrow_mut();
                *polls += 1;
                if *polls <= self.pending_until {
                    Ok(br#"{"error":"authorization_pending"}"#.to_vec())
                } else {
                    Ok(br#"{"access_token":"ms-token"}"#.to_vec())
                }
            }
            auth::XBOX_USER => Ok(br#"{"Token":"xbl","DisplayClaims":{"xui":[{"uhs":"hash1"}]}}"#.to_vec()),
            auth::XBOX_XSTS => Ok(br#"{"Token":"xsts","DisplayClaims":{"xui":[{"uhs":"hash2"}]}}"#.to_vec()),
            auth::MINECRAFT_LOGIN => Ok(br#"{"access_token":"mc-token"}"#.to_vec()),
            other => Err(format!("{other}: not in the fixture")),
        }
    }
}

fn main() {
    let client_id = "00000000-0000-0000-0000-000000000000";
    let transport = Fake::new(3);
    let mut signin = SignIn::begin(&transport, client_id).expect("a device code");

    // A frame loop: 60 Hz, nothing blocking, the clock is ours.
    let frame_ms: u64 = 16;
    let mut now_ms: u64 = 0;
    let mut next_tick_ms: u64 = 0;
    let deadline_ms: u64 = 120_000;
    let mut frames: u64 = 0;

    println!("t={:>6}ms  engine frame loop starts", now_ms);
    let microsoft_token = loop {
        frames += 1;
        now_ms += frame_ms;
        if now_ms > deadline_ms {
            panic!("the player never finished signing in");
        }
        if now_ms < next_tick_ms {
            continue;
        }
        match signin.tick(&transport, client_id).expect("a step") {
            Step::Show {
                user_code,
                verification_uri,
                interval,
            } => {
                println!("t={now_ms:>6}ms  show: type {user_code} at {verification_uri}");
                next_tick_ms = now_ms + interval * 1000;
            }
            Step::Wait { seconds } => {
                println!("t={now_ms:>6}ms  waiting: {seconds}s");
                next_tick_ms = now_ms + seconds * 1000;
            }
            Step::Done { microsoft_token } => {
                println!("t={now_ms:>6}ms  signed in");
                break microsoft_token;
            }
        }
    };

    let minecraft_token =
        auth::from_microsoft(&transport, &microsoft_token).expect("a Minecraft token");
    println!("t={now_ms:>6}ms  Minecraft token: {minecraft_token}");

    let owns = hzr_download::owns_game(&transport, &minecraft_token).expect("an answer");
    println!("t={now_ms:>6}ms  owns Java Edition: {owns}");

    // What the loop proves: it never asked more often than the interval allowed.
    let polls = *transport.polls.borrow();
    println!(
        "frames={frames}  polls={polls}  polls per minute={:.1}",
        polls as f64 * 60_000.0 / now_ms as f64
    );
    assert!(owns);
    assert_eq!(polls, 4, "one poll per interval, no busy waiting");
    let requests = transport.requests.borrow();
    println!("requests: {}", requests.len());
    for url in requests.iter() {
        println!("  {url}");
    }
}
