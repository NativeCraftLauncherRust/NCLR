//! `hzr-download` command line: get the player's own Minecraft files, verified.

use hzr_download::{
    download, resolve, root_files, version_json, HttpTransport, Plan, Report,
};
use hzr_fetch::AssetIndex;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
hzr-download: fetch Minecraft's own files from Mojang, on this device

Usage:
  hzr-download plan  [--version V] [--out DIR] [--store DIR]
  hzr-download fetch [--version V] [--out DIR] [--store DIR] [--no-assets] [--dry-run]
  hzr-download signin --client-id ID [--then-fetch] [--version V] [--out DIR] [--store DIR]

  plan    Print what would be fetched: the client jar, the asset index and every
          object, with their published sizes. Downloads nothing.
  fetch   Fetch it. Every file is checked against the SHA-1 the version JSON or
          the asset index publishes before it is written.

  --version  a version id, or `latest` (default: latest)
  --out      where client.jar goes   (default: <checkout>/route3/out/client)
  --store    the assets folder       (default: <checkout>/route3/out/assets)
             laid out Mojang's way: indexes/<id>.json and objects/<xx>/<hash>
  --no-assets   only the jar and the index, not the several thousand objects
  --dry-run     with fetch: print the plan and stop

  signin  Device-code sign-in: prints the code to type and the URL to open, then
          polls until Microsoft answers, then proves the account owns the game.
          --client-id  the Microsoft application id to sign in as (required)
          --then-fetch continue straight into a download once the token is in hand

Nothing here carries a game file: the bytes come from Mojang's CDN to the player
who owns the game.
";

struct Args {
    command: String,
    version: String,
    out: PathBuf,
    store: PathBuf,
    no_assets: bool,
    dry_run: bool,
    only: Vec<String>,
}

fn workspace(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative)
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        command: args.first().cloned().unwrap_or_default(),
        version: "latest".into(),
        out: workspace("route3/out/client"),
        store: workspace("route3/out/assets"),
        no_assets: false,
        dry_run: false,
        only: Vec::new(),
    };
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" => parsed.version = iter.next().ok_or("--version needs an id")?.clone(),
            "--out" => parsed.out = PathBuf::from(iter.next().ok_or("--out needs a folder")?),
            "--store" => parsed.store = PathBuf::from(iter.next().ok_or("--store needs a folder")?),
            "--no-assets" => parsed.no_assets = true,
            "--only" => {
                let list = iter.next().ok_or("--only needs a list")?;
                parsed.only = list.split(',').map(|part| part.trim().to_string()).collect();
            }
            "--dry-run" => parsed.dry_run = true,
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    Ok(parsed)
}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("hzr-download: error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some(first) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(ExitCode::from(2));
    };
    if matches!(first, "-h" | "--help" | "help") {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    // `signin` has its own flags; the shared parser would reject them.
    if first == "signin" {
        return signin(&args[1..]);
    }
    let parsed = parse(args)?;
    match parsed.command.as_str() {
        "plan" | "fetch" => {
            let dry = parsed.dry_run || parsed.command == "plan";
            go(&parsed, dry)
        }

        other => Err(format!("unrecognized command {other}\n\n{HELP}")),
    }
}

fn print_plan(label: &str, plan: &Plan) {
    println!(
        "{label}: {} files, {} bytes ({:.1} MiB)",
        plan.files.len(),
        plan.total_bytes(),
        plan.total_bytes() as f64 / (1u64 << 20) as f64
    );
}

fn go(parsed: &Args, dry_run: bool) -> Result<ExitCode, String> {
    let transport = HttpTransport::new();
    let reference = resolve(&transport, &parsed.version)?;
    println!("version     {} ({})", reference.id, reference.kind);
    let version = version_json(&transport, &reference)?;
    std::fs::create_dir_all(&parsed.out).map_err(|e| format!("{}: {e}", parsed.out.display()))?;

    let root = root_files(&version, &parsed.out, &parsed.store);
    print_plan("root", &root);

    let mut report = Report::default();
    if dry_run {
        for file in &root.files {
            println!("  {} {} ({} bytes)", file.label, file.path.display(), file.size);
        }
    } else {
        report = download(&transport, &root, |file, _| {
            println!("  got {}", file.label);
        });
        finish(&report)?;
    }

    if parsed.no_assets {
        return Ok(ExitCode::SUCCESS);
    }

    let index_path = parsed
        .store
        .join("indexes")
        .join(format!("{}.json", version.asset_index.id));
    if dry_run {
        println!(
            "assets      would read {} ({} bytes) once fetched",
            index_path.display(),
            version.asset_index.size
        );
        return Ok(ExitCode::SUCCESS);
    }
    let bytes = std::fs::read(&index_path).map_err(|e| format!("{}: {e}", index_path.display()))?;
    if bytes.len() as u64 != version.asset_index.size
        || hzr_fetch::sha1_hex(&bytes) != version.asset_index.sha1
    {
        return Err(format!(
            "{}: does not match the index the version publishes",
            index_path.display()
        ));
    }
    let index = AssetIndex::parse(&bytes)?;
    let wanted = wanted_subtrees(&parsed.only)?;
    let objects = hzr_download::plan::object_files_where(&index, &parsed.store, wanted);
    print_plan("assets", &objects);

    let mut seen = 0usize;
    let objects_report = download(&transport, &objects, |_, _| {
        seen += 1;
        if seen % 500 == 0 {
            println!("  {seen}/{} objects", objects.files.len());
        }
    });
    finish(&objects_report)?;

    println!(
        "done: {} downloaded, {} already present, {} bytes written",
        report.downloaded + objects_report.downloaded,
        report.skipped + objects_report.skipped,
        report.bytes + objects_report.bytes
    );
    Ok(ExitCode::SUCCESS)
}

/// The asset subtrees `--only` names, as one predicate.
fn wanted_subtrees(only: &[String]) -> Result<impl Fn(&str) -> bool, String> {
    let mut prefixes: Vec<&'static str> = Vec::new();
    for name in only.iter().filter(|name| name.as_str() != "all") {
        match name.as_str() {
            "fonts" => prefixes.push("minecraft/font/"),
            "languages" => prefixes.push("minecraft/lang/"),
            "panorama" => prefixes.push("minecraft/textures/gui/title/background/"),
            "packs" => prefixes.push("minecraft/resourcepacks/"),
            "sounds" => prefixes.push("minecraft/sounds/"),
            other => return Err(format!("unknown asset subtree {other}")),
        }
    }
    Ok(move |name: &str| {
        // No filter at all means the whole store.
        prefixes.is_empty()
            || prefixes.iter().any(|prefix| name.starts_with(prefix))
            // `pack.mcmeta` is the language step's marker file.
            || name == "pack.mcmeta"
    })
}

fn finish(report: &Report) -> Result<(), String> {
    if report.is_clean() {
        return Ok(());
    }
    for (file, why) in &report.failures {
        eprintln!("  failed {file}: {why}");
    }
    Err(format!("{} files did not arrive", report.failures.len()))
}

/// `signin`: the device-code flow, printing what the player must do.
///
/// The app drives the same state machine without sleeping; this loop exists so a
/// person can run the flow from a terminal before any UI exists.
fn signin(args: &[String]) -> Result<ExitCode, String> {
    let mut client_id = None;
    let mut then_fetch = false;
    let mut version = "26.3".to_string();
    let mut out = PathBuf::from("mc");
    let mut store = PathBuf::from("assets");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--client-id" => client_id = Some(iter.next().ok_or("--client-id needs a value")?.clone()),
            "--then-fetch" => then_fetch = true,
            "--version" => version = iter.next().ok_or("--version needs a value")?.clone(),
            "--out" => out = PathBuf::from(iter.next().ok_or("--out needs a folder")?),
            "--store" => store = PathBuf::from(iter.next().ok_or("--store needs a folder")?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let client_id = client_id.ok_or("signin needs --client-id")?;
    let transport = HttpTransport::new();
    let mut signin = hzr_download::auth::SignIn::begin(&transport, &client_id)?;

    let token = loop {
        match signin.tick(&transport, &client_id)? {
            hzr_download::auth::Step::Show {
                user_code,
                verification_uri,
                interval,
            } => {
                println!("open {verification_uri}");
                println!("and enter: {user_code}");
                println!("polling every {interval}s");
            }
            hzr_download::auth::Step::Wait { seconds } => {
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            }
            hzr_download::auth::Step::Done { microsoft_token } => {
                println!("signed in");
                break hzr_download::auth::from_microsoft(&transport, &microsoft_token)?;
            }
        }
    };

    // The token is what makes a download legitimate; the fetch reads it from here.
    std::env::set_var("HORIZON_ACCESS_TOKEN", &token);
    let owned = hzr_download::owns_game(&transport, &token)?;
    println!("owns Minecraft: Java Edition: {owned}");
    if !owned {
        return Err("this account does not own the game".into());
    }
    if !then_fetch {
        println!("token acquired; run `fetch` with HORIZON_ACCESS_TOKEN set to download");
        return Ok(ExitCode::SUCCESS);
    }
    let reference = resolve(&transport, &version)?;
    let json = version_json(&transport, &reference)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::create_dir_all(&store).map_err(|e| format!("{}: {e}", store.display()))?;
    let root = root_files(&json, &out, &store);
    let report = download(&transport, &root, |_, _| {});
    if !report.is_clean() {
        return Err(format!("{} files did not arrive", report.failures.len()));
    }
    let index_path = store
        .join("indexes")
        .join(format!("{}.json", json.asset_index.id));
    let index = AssetIndex::parse(
        &std::fs::read(&index_path).map_err(|e| format!("{}: {e}", index_path.display()))?,
    )?;
    let objects = hzr_download::plan::object_files_where(&index, &store, |name| {
        name.starts_with("minecraft/font/")
            || name.starts_with("minecraft/lang/")
            || name.starts_with("minecraft/textures/gui/title/background/")
            || name.starts_with("minecraft/resourcepacks/")
            || name == "pack.mcmeta"
    });
    let report = download(&transport, &objects, |_, _| {});
    if !report.is_clean() {
        return Err(format!("{} assets did not arrive", report.failures.len()));
    }
    println!("downloaded into {} and {}", out.display(), store.display());
    Ok(ExitCode::SUCCESS)
}