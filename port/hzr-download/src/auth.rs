//! Signing in, far enough to prove the account owns the game.
//!
//! The device-code flow: the app shows a code and a URL, the player types it on
//! any browser, and the app polls until Microsoft answers. No redirect URL, no
//! embedded browser - which is what makes it workable on iOS.
//!
//! ```text
//! devicecode -> user_code + verification_uri      (what the app shows)
//! token      -> Microsoft access token            (poll; pending until typed)
//! xbox       -> Xbox Live token + user hash
//! xsts       -> XSTS token + user hash
//! login_xbox -> Minecraft access token            (what owns_game checks)
//! ```
//!
//! Every step is one POST, so the whole chain is exercised against a fixture
//! without a network.

use crate::fetch::Transport;
use serde::Deserialize;

/// Microsoft's consumer tenant, where personal accounts sign in.
pub const DEVICE_CODE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
pub const TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
pub const XBOX_USER: &str = "https://user.auth.xboxlive.com/user/authenticate";
pub const XBOX_XSTS: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
pub const MINECRAFT_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";

/// The scope that lets an app read Xbox Live and Minecraft identity.
pub const SCOPE: &str = "XboxLive.signin offline_access";

/// What the player is told to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The app polls with this; never shown.
    pub device_code: String,
    /// The short code the player types.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// Seconds between polls, as Microsoft asks.
    pub interval: u64,
    /// Seconds until the code stops working.
    pub expires_in: u64,
}

/// One step's answer: done, still waiting, or a failure.
#[derive(Debug)]
pub enum Poll {
    Token(String),
    Pending,
    SlowDown,
}

#[derive(Deserialize)]
struct DeviceCodeBody {
    device_code: String,
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    #[serde(default)]
    interval: u64,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenBody {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct XboxBody {
    #[serde(rename = "Token", default)]
    token: String,
    #[serde(rename = "DisplayClaims", default)]
    claims: DisplayClaims,
}

#[derive(Deserialize, Default)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<Xui>,
}

#[derive(Deserialize, Default)]
struct Xui {
    #[serde(default)]
    uhs: String,
}

#[derive(Deserialize)]
struct MinecraftBody {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "errorMessage")]
    error_message: Option<String>,
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encoding for the form bodies: enough for the values we send.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("{what}: {e}"))
}

/// Ask Microsoft for a code to show the player.
pub fn start<T: Transport>(transport: &T, client_id: &str) -> Result<DeviceCode, String> {
    let body = form(&[("client_id", client_id), ("scope", SCOPE)]);
    let bytes = transport
        .post(DEVICE_CODE, "application/x-www-form-urlencoded", &body)
        .map_err(|error| format!("device code: {error}"))?;
    let parsed: DeviceCodeBody = parse(&bytes, "device code")?;
    if parsed.user_code.is_empty() || parsed.device_code.is_empty() {
        return Err("device code: the answer carried no code".into());
    }
    Ok(DeviceCode {
        device_code: parsed.device_code,
        user_code: parsed.user_code,
        verification_uri: if parsed.verification_uri.is_empty() {
            "https://microsoft.com/link".to_string()
        } else {
            parsed.verification_uri
        },
        interval: parsed.interval.max(1),
        expires_in: parsed.expires_in,
    })
}

/// Poll once. `Pending` means the player has not typed the code yet.
pub fn poll<T: Transport>(
    transport: &T,
    client_id: &str,
    device_code: &str,
) -> Result<Poll, String> {
    let body = form(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", client_id),
        ("device_code", device_code),
    ]);
    let bytes = transport
        .post(TOKEN, "application/x-www-form-urlencoded", &body)
        .map_err(|error| format!("token: {error}"))?;
    let parsed: TokenBody = parse(&bytes, "token")?;
    if let Some(token) = parsed.access_token {
        return Ok(Poll::Token(token));
    }
    match parsed.error.as_deref() {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some(other) => Err(format!(
            "token: {other}{}",
            parsed
                .error_description
                .map(|text| format!(" ({text})"))
                .unwrap_or_default()
        )),
        None => Err("token: the answer carried neither a token nor an error".into()),
    }
}

fn xbox_identity(token: &str) -> String {
    format!(r#"{{"Properties":{{"AuthMethod":"RPS","SiteName":"user.auth.xboxlive.com","RpsTicket":"d={token}"}},"RelyingParty":"http://auth.xboxlive.com","TokenType":"JWT"}}"#)
}

fn xsts_body(token: &str) -> String {
    format!(r#"{{"Properties":{{"SandboxId":"RETAIL","UserTokens":["{token}"]}},"RelyingParty":"rp://api.minecraftservices.com/","TokenType":"JWT"}}"#)
}

/// Microsoft token -> Xbox Live token and user hash.
pub fn xbox<T: Transport>(transport: &T, microsoft_token: &str) -> Result<(String, String), String> {
    let bytes = transport
        .post(XBOX_USER, "application/json", &xbox_identity(microsoft_token))
        .map_err(|error| format!("xbox: {error}"))?;
    let parsed: XboxBody = parse(&bytes, "xbox")?;
    let hash = parsed
        .claims
        .xui
        .first()
        .map(|xui| xui.uhs.clone())
        .unwrap_or_default();
    if parsed.token.is_empty() || hash.is_empty() {
        return Err("xbox: the answer carried no token or no user hash".into());
    }
    Ok((parsed.token, hash))
}

/// Xbox Live token -> XSTS token and user hash.
pub fn xsts<T: Transport>(transport: &T, xbox_token: &str) -> Result<(String, String), String> {
    let bytes = transport
        .post(XBOX_XSTS, "application/json", &xsts_body(xbox_token))
        .map_err(|error| format!("xsts: {error}"))?;
    let parsed: XboxBody = parse(&bytes, "xsts")?;
    let hash = parsed
        .claims
        .xui
        .first()
        .map(|xui| xui.uhs.clone())
        .unwrap_or_default();
    if parsed.token.is_empty() || hash.is_empty() {
        // A child account or one without an Xbox profile answers with no token.
        return Err(
            "xsts: no token; the account may have no Xbox profile, or needs an adult".into(),
        );
    }
    Ok((parsed.token, hash))
}

/// XSTS  - the Minecraft access token `owns_game` and the launcher use.
pub fn minecraft<T: Transport>(
    transport: &T,
    xsts_token: &str,
    user_hash: &str,
) -> Result<String, String> {
    let body = format!(r#"{{"identityToken":"XBL3.0 x={user_hash};{xsts_token}"}}"#);
    let bytes = transport
        .post(MINECRAFT_LOGIN, "application/json", &body)
        .map_err(|error| format!("minecraft: {error}"))?;
    let parsed: MinecraftBody = parse(&bytes, "minecraft")?;
    match parsed.access_token {
        Some(token) if !token.is_empty() => Ok(token),
        _ => Err(format!(
            "minecraft: {}",
            parsed
                .error_message
                .or(parsed.error)
                .unwrap_or_else(|| "no access token".into())
        )),
    }
}

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Show this to the player; then call [`SignIn::tick`] again.
    Show {
        user_code: String,
        verification_uri: String,
        interval: u64,
    },
    /// Nothing to do yet; ask again in `seconds`.
    Wait { seconds: u64 },
    /// Signed in: the Microsoft token is ready for [`from_microsoft`].
    Done { microsoft_token: String },
}

/// A sign-in in progress, driven one step at a time.
///
/// The caller owns the clock: the app shows the code, then calls `tick` on its
/// own timer. Nothing here sleeps, so a slow sign-in never blocks a UI thread
/// and the state machine is testable without waiting.
pub struct SignIn {
    code: DeviceCode,
    shown: bool,
    interval: u64,
}

impl SignIn {
    /// Ask Microsoft for a code, without showing anything yet.
    pub fn begin<T: Transport>(transport: &T, client_id: &str) -> Result<SignIn, String> {
        let code = start(transport, client_id)?;
        Ok(SignIn {
            interval: code.interval,
            code,
            shown: false,
        })
    }

    /// The code the player must type, once.
    pub fn code(&self) -> &DeviceCode {
        &self.code
    }

    /// One step. The first call only says what to show; later ones poll.
    pub fn tick<T: Transport>(&mut self, transport: &T, client_id: &str) -> Result<Step, String> {
        if !self.shown {
            self.shown = true;
            return Ok(Step::Show {
                user_code: self.code.user_code.clone(),
                verification_uri: self.code.verification_uri.clone(),
                interval: self.interval,
            });
        }
        match poll(transport, client_id, &self.code.device_code)? {
            Poll::Pending => Ok(Step::Wait {
                seconds: self.interval,
            }),
            Poll::SlowDown => {
                // Microsoft asks for more room: double the gap, bounded.
                self.interval = (self.interval * 2).min(30);
                Ok(Step::Wait {
                    seconds: self.interval,
                })
            }
            Poll::Token(microsoft_token) => Ok(Step::Done { microsoft_token }),
        }
    }
}

/// The whole chain from a Microsoft token, for a caller that already has one.
pub fn from_microsoft<T: Transport>(transport: &T, microsoft_token: &str) -> Result<String, String> {
    let (xbox_token, _) = xbox(transport, microsoft_token)?;
    let (xsts_token, hash) = xsts(transport, &xbox_token)?;
    minecraft(transport, &xsts_token, &hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Answers each URL with a canned body and records the requests.
    struct Fake {
        answers: RefCell<Vec<(String, String)>>,
        seen: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(answers: &[(&str, &str)]) -> Fake {
            Fake {
                answers: RefCell::new(
                    answers
                        .iter()
                        .map(|(url, body)| ((*url).to_string(), (*body).to_string()))
                        .collect(),
                ),
                seen: RefCell::new(Vec::new()),
            }
        }
        fn take(&self, url: &str) -> Result<Vec<u8>, String> {
            self.seen.borrow_mut().push(url.to_string());
            let answers = self.answers.borrow();
            answers
                .iter()
                .find(|(candidate, _)| candidate == url)
                .map(|(_, body)| body.clone().into_bytes())
                .ok_or_else(|| format!("{url}: not in the fixture"))
        }
    }

    impl Transport for Fake {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.take(url)
        }
        fn post(&self, url: &str, _kind: &str, _body: &str) -> Result<Vec<u8>, String> {
            self.take(url)
        }
    }

    #[test]
    fn the_device_code_gives_what_the_app_shows() {
        let fake = Fake::new(&[(
            DEVICE_CODE,
            r#"{"device_code":"dc","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/link","interval":5,"expires_in":900}"#,
        )]);
        let code = start(&fake, "client").unwrap();
        assert_eq!(code.user_code, "ABCD-EFGH");
        assert_eq!(code.verification_uri, "https://microsoft.com/link");
        assert_eq!(code.interval, 5);
        assert_eq!(code.expires_in, 900);
    }

    #[test]
    fn an_interval_of_zero_is_never_polled_at_zero() {
        let fake = Fake::new(&[(
            DEVICE_CODE,
            r#"{"device_code":"dc","user_code":"X","interval":0}"#,
        )]);
        assert_eq!(start(&fake, "c").unwrap().interval, 1);
    }

    #[test]
    fn a_pending_poll_is_not_a_failure() {
        let fake = Fake::new(&[(
            TOKEN,
            r#"{"error":"authorization_pending","error_description":"waiting"}"#,
        )]);
        assert!(matches!(poll(&fake, "c", "dc").unwrap(), Poll::Pending));
    }

    #[test]
    fn slow_down_asks_the_caller_to_wait() {
        let fake = Fake::new(&[(TOKEN, r#"{"error":"slow_down"}"#)]);
        assert!(matches!(poll(&fake, "c", "dc").unwrap(), Poll::SlowDown));
    }

    #[test]
    fn an_expired_code_is_an_error() {
        let fake = Fake::new(&[(
            TOKEN,
            r#"{"error":"expired_token","error_description":"the code expired"}"#,
        )]);
        let error = poll(&fake, "c", "dc").unwrap_err();
        assert!(error.contains("expired_token"), "{error}");
    }

    #[test]
    fn an_answer_with_neither_a_token_nor_an_error_is_refused() {
        let fake = Fake::new(&[(TOKEN, r#"{}"#)]);
        assert!(poll(&fake, "c", "dc").is_err());
    }

    #[test]
    fn the_xbox_and_xsts_steps_carry_the_user_hash_through() {
        let fake = Fake::new(&[
            (
                XBOX_USER,
                r#"{"Token":"xbl","DisplayClaims":{"xui":[{"uhs":"hash1"}]}}"#,
            ),
            (
                XBOX_XSTS,
                r#"{"Token":"xsts","DisplayClaims":{"xui":[{"uhs":"hash2"}]}}"#,
            ),
            (MINECRAFT_LOGIN, r#"{"access_token":"mc-token"}"#),
        ]);
        let (xbox_token, hash) = xbox(&fake, "ms").unwrap();
        assert_eq!((xbox_token.as_str(), hash.as_str()), ("xbl", "hash1"));
        let (xsts_token, hash) = xsts(&fake, &xbox_token).unwrap();
        assert_eq!((xsts_token.as_str(), hash.as_str()), ("xsts", "hash2"));
        assert_eq!(minecraft(&fake, &xsts_token, &hash).unwrap(), "mc-token");
    }

    #[test]
    fn an_account_without_an_xbox_profile_is_told_so() {
        let fake = Fake::new(&[(XBOX_XSTS, r#"{"Token":"","DisplayClaims":{"xui":[]}}"#)]);
        let error = xsts(&fake, "xbl").unwrap_err();
        assert!(error.contains("Xbox profile"), "{error}");
    }

    #[test]
    fn a_refused_minecraft_login_names_its_reason() {
        let fake = Fake::new(&[(
            MINECRAFT_LOGIN,
            r#"{"error":"FORBIDDEN","errorMessage":"Invalid identity token"}"#,
        )]);
        let error = minecraft(&fake, "x", "h").unwrap_err();
        assert!(error.contains("Invalid identity token"), "{error}");
    }

    #[test]
    fn the_poll_body_is_a_form_the_token_endpoint_accepts() {
        assert_eq!(
            form(&[("grant_type", "urn:x"), ("device_code", "a b")]),
            "grant_type=urn%3Ax&device_code=a%20b"
        );
        assert_eq!(encode("XboxLive.signin offline_access"), "XboxLive.signin%20offline_access");
    }
}

#[cfg(test)]
mod signin_tests {
    use super::*;
    use std::cell::RefCell;

    struct Script {
        /// One answer per poll, in order: "pending", "slow_down" or a token.
        polls: RefCell<Vec<String>>,
    }

    impl Transport for Script {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            Err(format!("{url}: unexpected GET"))
        }
        fn post(&self, url: &str, _kind: &str, _body: &str) -> Result<Vec<u8>, String> {
            if url == DEVICE_CODE {
                return Ok(br#"{"device_code":"dc","user_code":"CODE","verification_uri":"https://microsoft.com/link","interval":5,"expires_in":900}"#.to_vec());
            }
            let mut polls = self.polls.borrow_mut();
            let next = polls.remove(0);
            Ok(match next.as_str() {
                "pending" => br#"{"error":"authorization_pending"}"#.to_vec(),
                "slow_down" => br#"{"error":"slow_down"}"#.to_vec(),
                token => format!(r#"{{"access_token":"{token}"}}"#).into_bytes(),
            })
        }
    }

    fn script(polls: &[&str]) -> Script {
        Script {
            polls: RefCell::new(polls.iter().map(|p| (*p).to_string()).collect()),
        }
    }

    #[test]
    fn the_first_step_is_what_to_show() {
        let transport = script(&["pending"]);
        let mut signin = SignIn::begin(&transport, "client").unwrap();
        assert_eq!(
            signin.tick(&transport, "client").unwrap(),
            Step::Show {
                user_code: "CODE".into(),
                verification_uri: "https://microsoft.com/link".into(),
                interval: 5,
            }
        );
    }

    #[test]
    fn waiting_repeats_the_interval_and_ends_with_the_token() {
        let transport = script(&["pending", "pending", "ms-token"]);
        let mut signin = SignIn::begin(&transport, "client").unwrap();
        assert!(matches!(signin.tick(&transport, "client").unwrap(), Step::Show { .. }));
        assert_eq!(
            signin.tick(&transport, "client").unwrap(),
            Step::Wait { seconds: 5 }
        );
        assert_eq!(
            signin.tick(&transport, "client").unwrap(),
            Step::Wait { seconds: 5 }
        );
        assert_eq!(
            signin.tick(&transport, "client").unwrap(),
            Step::Done {
                microsoft_token: "ms-token".into()
            }
        );
    }

    #[test]
    fn slow_down_widens_the_gap_and_keeps_it_bounded() {
        let many: Vec<&str> = std::iter::repeat("slow_down").take(8).collect();
        let mut scripted = many.clone();
        scripted.push("tok");
        let transport = script(&scripted);
        let mut signin = SignIn::begin(&transport, "client").unwrap();
        let _ = signin.tick(&transport, "client").unwrap();
        let mut gaps = Vec::new();
        for _ in 0..8 {
            match signin.tick(&transport, "client").unwrap() {
                Step::Wait { seconds } => gaps.push(seconds),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(gaps, vec![10, 20, 30, 30, 30, 30, 30, 30]);
    }

    #[test]
    fn an_expired_code_stops_the_machine_with_an_error() {
        let transport = script(&["expired"]);
        struct Expired;
        impl Transport for Expired {
            fn fetch(&self, _: &str) -> Result<Vec<u8>, String> {
                Ok(Vec::new())
            }
            fn post(&self, url: &str, _: &str, _: &str) -> Result<Vec<u8>, String> {
                if url == DEVICE_CODE {
                    return Ok(br#"{"device_code":"dc","user_code":"C","interval":5}"#.to_vec());
                }
                Ok(br#"{"error":"expired_token"}"#.to_vec())
            }
        }
        let mut signin = SignIn::begin(&Expired, "c").unwrap();
        let _ = signin.tick(&Expired, "c").unwrap();
        let error = signin.tick(&Expired, "c").unwrap_err();
        assert!(error.contains("expired_token"), "{error}");
        let _ = transport;
    }
}
