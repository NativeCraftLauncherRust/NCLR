//! C ABI used by the iOS UIKit shell.
//!
//! This first milestone only reports what was compiled and linked. The game
//! entry point will replace it once the client library builds for iOS.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::sync::OnceLock;

static DIAGNOSTICS: OnceLock<CString> = OnceLock::new();

/// Returns a NUL-terminated UTF-8 diagnostics string owned by this library.
///
/// The pointer stays valid for the lifetime of the process.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_diagnostics() -> *const c_char {
    DIAGNOSTICS
        .get_or_init(|| {
            let mappings = horizon_client_lib::keybinds::all_mappings().count();
            let text = format!(
                "Horizon iOS development shell\n\
                 Rust core: {}\n\
                 Minecraft target: {}\n\
                 Build arch: {}\n\
                 Input mappings: {mappings}\n\
                 Status: client library linked, game entry not wired yet",
                env!("CARGO_PKG_VERSION"),
                horizon_core::VANILLA_VERSION,
                std::env::consts::ARCH,
            );
            CString::new(text).expect("diagnostics contain no NUL")
        })
        .as_ptr()
}

/// A trivial linked-core check the UIKit shell can display.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_self_test() -> i32 {
    // Read a real value from horizon-core so the linker cannot drop the crate.
    if horizon_core::VANILLA_VERSION.is_empty() {
        1
    } else {
        0
    }
}

/// Starts the real game. The caller must not have called `UIApplicationMain`:
/// winit's iOS backend owns that call and the app's lifecycle.
///
/// `resources` must point at a directory prepared by `horizon-prepare`;
/// `game_dir` must be writable (the app's Documents directory).
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_run(resources: *const c_char, game_dir: *const c_char) -> i32 {
    if resources.is_null() || game_dir.is_null() {
        return 2;
    }
    let resources = match unsafe { CStr::from_ptr(resources) }.to_str() {
        Ok(path) => PathBuf::from(path),
        Err(_) => return 2,
    };
    let game_dir = match unsafe { CStr::from_ptr(game_dir) }.to_str() {
        Ok(path) => PathBuf::from(path),
        Err(_) => return 2,
    };
    let startup = horizon_client::entry::prepare_ios(resources, game_dir).unwrap_or_else(|error| {
        eprintln!("Horizon: {error}");
        std::process::exit(2);
    });
    if let Err(error) = horizon_engine_slots::install_seal(mod_points::__private::seal) {
        eprintln!("Horizon: engine seal unavailable: {error}");
        return 3;
    }
    if let Err(error) = mod_api::__private::initialize() {
        eprintln!("Horizon: mod instrumentation unavailable: {error}");
        return 3;
    }
    if let Err(error) = horizon_engine_slots::install_platform(horizon_engine_slots::Platform {
        install_packages: mod_api::__private::install_packages,
        install_registration_data: mod_api::__private::install_registration_data,
        install_parts: mod_api::__private::install_parts,
        profiler_is_enabled: mod_api::__private::profiler::is_enabled,
        profiler_start: mod_api::__private::profiler::start,
        profiler_stop: mod_api::__private::profiler::stop,
        profiler_summary: mod_api::__private::profiler::summary,
        profiler_firefox_profile: mod_api::__private::profiler::firefox_profile,
    }) {
        eprintln!("Horizon: engine platform unavailable: {error}");
        return 3;
    }
    mod_render::render::install();
    horizon_client::entry::start(
        horizon_mod_package::runtime::Runtime::default(),
        |_, _| (Vec::new(), ()),
        horizon_client::entry::CodeRuntime {
            initialize: |_| {},
            refusal: |_| None,
            startup_refused: |_| false,
        },
        startup,
    );
    0
}
/// The resource tables a prepared folder cannot boot without (`route3/AUDIT.md`).
///
/// Only the ones the device cannot derive: `assets/`, `data/` and most of
/// `worldgen/` are produced from the player's own jar.
const REQUIRED_TABLES: [&str; 16] = [
    "facts.json",
    "gameplay-facts.json",
    "block-behaviours.json",
    "model-states.json",
    "occlusion.nbt",
    "cached-collision.nbt",
    "movement-properties.nbt",
    "render-definitions.json",
    "attributes.json",
    "landing-definitions.json",
    "items.json",
    "entity-types.json",
    "command-registries.json",
    "commands.json",
    "registries.json",
    "player-float-ids.json",
];

/// Why `resources` cannot boot, or null when it can.
///
/// The UIKit shell calls this before `horizon_ios_run`, so a half-copied folder
/// names what is missing instead of failing inside the loader. The returned
/// string is owned by this library and never freed.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_check_resources(resources: *const c_char) -> *const c_char {
    if resources.is_null() {
        // A NULL path is a caller bug, not a prepared folder: say so instead of
        // answering "fine" and letting the loader fail later.
        return CString::new("resources path is null")
            .expect("no NUL")
            .into_raw();
    }
    let Ok(text) = (unsafe { CStr::from_ptr(resources) }).to_str() else {
        return CString::new("resources path is not UTF-8")
            .expect("no NUL")
            .into_raw();
    };
    let root = std::path::Path::new(text);
    let mut gaps: Vec<String> = Vec::new();
    if !root.join("horizon-resources.json").is_file() {
        gaps.push("horizon-resources.json is missing: the folder was never prepared".to_owned());
    }
    if !root.join("horizon-prepare.json").is_file() {
        gaps.push("horizon-prepare.json is missing: another preparer made this folder".to_owned());
    }
    for name in REQUIRED_TABLES {
        if !root.join(name).is_file() {
            gaps.push(name.to_owned());
        }
    }
    if !root.join("assets").is_dir() {
        gaps.push("assets/ is missing".to_owned());
    }
    if gaps.is_empty() {
        return std::ptr::null();
    }
    match CString::new(gaps.join("; ")) {
        Ok(message) => message.into_raw() as *const c_char,
        Err(_) => std::ptr::null(),
    }
}


/// Assembles a resource tree from a shipped bundle and the player's own copy.
///
/// `hzr-device` does the work (`assemble_clean`); this is the C ABI the UIKit
/// shell calls before `horizon_ios_run`. Returns null on success, otherwise a
/// message owned by this library.
///
/// This needs `hzr-device` vendored at `crates/hzr-device` and depended on by
/// this crate; see `route3/patches/002-NOTES.md`.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_assemble(
    bundle: *const c_char,
    client_jar: *const c_char,
    index: *const c_char,
    store: *const c_char,
    out: *const c_char,
) -> *const c_char {
    let text = |pointer: *const c_char| -> Option<String> {
        if pointer.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    let (Some(bundle), Some(client_jar), Some(index), Some(store), Some(out)) = (
        text(bundle),
        text(client_jar),
        text(index),
        text(store),
        text(out),
    ) else {
        return message("assemble needs five UTF-8 paths");
    };
    match hzr_device::assemble_clean(
        std::path::Path::new(&bundle),
        std::path::Path::new(&client_jar),
        std::path::Path::new(&index),
        std::path::Path::new(&store),
        std::path::Path::new(&out),
    ) {
        Ok(_) => std::ptr::null(),
        Err(error) => message(&error),
    }
}

/// A C string this library owns for the rest of the process.
fn message(text: &str) -> *const c_char {
    CString::new(text.replace('\0', " "))
        .map(|value| value.into_raw() as *const c_char)
        .unwrap_or(std::ptr::null())
}

/// Downloads the player's own Minecraft from Mojang: the version's jar, its asset
/// index, and the asset subtrees the assembly imports.
///
/// `hzr-download` does the work, verifying every file against the SHA-1 the
/// version JSON or the asset index publishes. Returns null on success, otherwise
/// a message owned by this library.
///
/// The download needs no account: Mojang's CDN is public. Checking that the
/// player owns the game is the shell's business, before it calls this.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_fetch(
    version: *const c_char,
    jar_dir: *const c_char,
    store_dir: *const c_char,
) -> *const c_char {
    let text = |pointer: *const c_char| -> Option<String> {
        if pointer.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    let (Some(version), Some(jar_dir), Some(store_dir)) =
        (text(version), text(jar_dir), text(store_dir))
    else {
        return message("fetch needs three UTF-8 paths");
    };
    match fetch_into(&version, &jar_dir, &store_dir) {
        Ok(()) => std::ptr::null(),
        Err(error) => message(&error),
    }
}

fn fetch_into(version: &str, jar_dir: &str, store_dir: &str) -> Result<(), String> {
    use std::path::Path;
    let jar_dir = Path::new(jar_dir);
    let store = Path::new(store_dir);
    std::fs::create_dir_all(jar_dir).map_err(|e| format!("{}: {e}", jar_dir.display()))?;
    std::fs::create_dir_all(store).map_err(|e| format!("{}: {e}", store.display()))?;

    let transport = hzr_download::HttpTransport::new();
    // The download itself is public. A release build sets HORIZON_ACCESS_TOKEN
    // once the player signs in, and then a first run has to prove ownership
    // before a single byte is fetched.
    if let Ok(token) = std::env::var("HORIZON_ACCESS_TOKEN") {
        let token = token.trim();
        if !token.is_empty() && !hzr_download::owns_game(&transport, token)? {
            return Err("this account does not own Minecraft: Java Edition".into());
        }
    }
    let reference = hzr_download::resolve(&transport, version)?;
    let json = hzr_download::version_json(&transport, &reference)?;

    // The jar and the asset index first: the index lists the objects.
    let root = hzr_download::root_files(&json, jar_dir, store);
    let report = hzr_download::download(&transport, &root, |_, _| {});
    if !report.is_clean() {
        let (what, why) = &report.failures[0];
        return Err(format!("{what}: {why}"));
    }

    let index_path = store
        .join("indexes")
        .join(format!("{}.json", json.asset_index.id));
    let bytes = std::fs::read(&index_path).map_err(|e| format!("{}: {e}", index_path.display()))?;
    let index = hzr_fetch::AssetIndex::parse(&bytes)?;
    // Only what the assembly imports: fonts, languages, the panorama and the
    // built-in packs. Everything else comes out of the jar.
    let objects = hzr_download::plan::object_files_where(&index, store, |name| {
        name.starts_with("minecraft/font/")
            || name.starts_with("minecraft/lang/")
            || name.starts_with("minecraft/textures/gui/title/background/")
            || name.starts_with("minecraft/resourcepacks/")
            || name == "pack.mcmeta"
    });
    let report = hzr_download::download(&transport, &objects, |_, _| {});
    if !report.is_clean() {
        let (what, why) = &report.failures[0];
        return Err(format!("{what}: {why}"));
    }
    Ok(())
}

/// Starts the device-code sign-in: the shell shows `user_code` and opens
/// `verification_uri`, then polls.
///
/// Answers one string, newline-separated, because the shell parses it without a
/// JSON dependency:
///
/// ```text
/// ok
/// <device_code>        kept by the shell, never shown
/// <user_code>          what the player types
/// <verification_uri>   where they type it
/// <interval>           seconds between polls
/// ```
///
/// or `error` and a message on the next line.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_auth_start(client_id: *const c_char) -> *const c_char {
    let Some(client_id) = text(client_id) else {
        return auth_message("auth: client id is not UTF-8");
    };
    let transport = hzr_download::HttpTransport::new();
    match hzr_download::auth::start(&transport, &client_id) {
        Ok(code) => auth_message(&format!(
            "ok\n{}\n{}\n{}\n{}",
            code.device_code, code.user_code, code.verification_uri, code.interval
        )),
        Err(error) => auth_message(&format!("error\n{}", error.replace(['\r', '\n'], " "))),
    }
}

/// Polls the token endpoint once.
///
/// Answers `pending`, `slow_down`, `token` and the token on the next line, or
/// `error` and a message on the next line.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_auth_poll(
    client_id: *const c_char,
    device_code: *const c_char,
) -> *const c_char {
    let (Some(client_id), Some(device_code)) = (text(client_id), text(device_code)) else {
        return auth_message("error\nauth: paths are not UTF-8");
    };
    let transport = hzr_download::HttpTransport::new();
    match hzr_download::auth::poll(&transport, &client_id, &device_code) {
        Ok(hzr_download::auth::Poll::Pending) => auth_message("pending"),
        Ok(hzr_download::auth::Poll::SlowDown) => auth_message("slow_down"),
        Ok(hzr_download::auth::Poll::Token(token)) => auth_message(&format!("token\n{token}")),
        Err(error) => auth_message(&format!("error\n{}", error.replace(['\r', '\n'], " "))),
    }
}

/// A Microsoft access token 鈫?the Minecraft access token, which is what the
/// player's identity and `owns_game` use. Answers `token` and the token on the
/// next line, or `error` and a message on the next line.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_auth_minecraft(microsoft_token: *const c_char) -> *const c_char {
    let Some(microsoft_token) = text(microsoft_token) else {
        return auth_message("error\nauth: the token is not UTF-8");
    };
    let transport = hzr_download::HttpTransport::new();
    match hzr_download::auth::from_microsoft(&transport, &microsoft_token) {
        Ok(token) => auth_message(&format!("token\n{token}")),
        Err(error) => auth_message(&format!("error\n{}", error.replace(['\r', '\n'], " "))),
    }
}

/// A NUL-terminated UTF-8 argument, or None.
fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// A string this library owns, exactly as built.
///
/// Newlines are the protocol here, so this must not touch them. Error messages
/// are flattened where they are built, not here.
fn auth_message(text: &str) -> *const c_char {
    CString::new(text.replace('\0', " "))
        .map(|value| value.into_raw() as *const c_char)
        .unwrap_or(std::ptr::null())
}

/// Deletes the downloads once the tree is assembled.
///
/// About 142 MiB that the player's device can have back, and that Mojang's CDN
/// can supply again. Refuses unless the folders look like the ones
/// `horizon_ios_fetch` wrote. Returns null on success, otherwise a message.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_ios_forget_downloads(
    jar_dir: *const c_char,
    store_dir: *const c_char,
) -> *const c_char {
    let (Some(jar_dir), Some(store_dir)) = (text(jar_dir), text(store_dir)) else {
        return message("clean needs two UTF-8 paths");
    };
    let jar = std::path::Path::new(&jar_dir);
    let store = std::path::Path::new(&store_dir);
    match hzr_device::forget_downloads(jar, store) {
        Ok(_) => std::ptr::null(),
        Err(error) => message(&error),
    }
}
