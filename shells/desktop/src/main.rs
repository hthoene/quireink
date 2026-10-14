//! QuireInk desktop emulator.
//!
//! Runs the real kernel and plugins against an emulated e-ink device —
//! interactively in a window, or headless for screenshots and CI.

mod budget;
mod device;
mod gui;
mod headless;
mod panel;
mod profiles;
mod storage;

use std::path::PathBuf;
use std::process::ExitCode;

use device::Device;

#[global_allocator]
static ALLOCATOR: budget::BudgetAllocator = budget::BudgetAllocator;

const USAGE: &str = "\
QuireInk emulator

USAGE:
    quireink-emu [OPTIONS]

OPTIONS:
    --profile <id>        Device to emulate (default: xteink-x4)
    --list-profiles       List available device profiles
    --config <file>       User configuration file (quireink.ini)
    --set <key>=<value>   Override configuration, e.g. --set core.language=de
                          or --set slots.core.home=core.placeholder
    --headless            Run without a window
    --script <steps>      Input steps, e.g. \"page-forward,long:confirm,tap:100:200\"
    --screenshot <file>   Save a PNG of the panel after the script (headless)
    --library <dir>       Folder emulating the device storage
                          (default: samples/library)
    --screenshot-dir <d>  Where F12 saves screenshots (default: out)
    --heap-budget <KiB>   Override the profile's heap budget (`none` = unlimited)
    --trace-alloc <bytes> Print every kernel allocation of at least this size
    -h, --help            Show this help

KEYS (if the device has the button):
    PageDown/Space  page-forward     PageUp      page-back
    Arrow keys      up/down/left/right
    Enter           confirm          Esc/Backspace back
    M               menu             H           home        P  power
    Hold a key for a long press. Mouse = touch screen.
    F2  show refreshed regions   F3  toggle flash animation   F12  screenshot
";

#[derive(Debug, Default)]
struct Options {
    profile: Option<String>,
    config: Option<PathBuf>,
    sets: Vec<(String, String)>,
    headless: bool,
    script: Option<String>,
    screenshot: Option<PathBuf>,
    screenshot_dir: Option<PathBuf>,
    library: Option<PathBuf>,
    /// `Some(None)` removes the budget.
    heap_budget: Option<Option<usize>>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(options) = parse_args(std::env::args().skip(1))? else {
        return Ok(());
    };
    let profile_id = options.profile.as_deref().unwrap_or("xteink-x4");
    let profile = profiles::find(profile_id)
        .ok_or_else(|| format!("unknown profile `{profile_id}` (see --list-profiles)"))?;

    let library = options
        .library
        .clone()
        .unwrap_or_else(|| PathBuf::from("samples/library"));
    let storage = storage::DirStorage::new(&library)
        .map_err(|e| format!("library {}: {e}", library.display()))?;
    let mut device = Device::new(profile, user_config(&options)?, storage, options.headless)
        .map_err(|e| format!("{}: {e}", profile.device.id))?;
    if let Some(limit) = options.heap_budget {
        budget::set_limit(limit);
    }

    if options.headless {
        device.start();
        if let Some(script) = &options.script {
            headless::run_script(&mut device, script)?;
        }
        if let Some(path) = &options.screenshot {
            headless::screenshot(&device, path)?;
            println!("screenshot saved to {}", path.display());
        }
        println!(
            "{}: screen {}, {}",
            profile.device.id,
            device
                .kernel
                .active_screen()
                .map(ToString::to_string)
                .unwrap_or_default(),
            budget::stats()
        );
        return Ok(());
    }

    if options.script.is_some() || options.screenshot.is_some() {
        return Err("--script and --screenshot require --headless".into());
    }
    let dir = options
        .screenshot_dir
        .unwrap_or_else(|| PathBuf::from("out"));
    gui::run(device, dir)
}

/// Reads `--config` and appends `--set` overrides as config sections.
fn user_config(options: &Options) -> Result<Option<String>, String> {
    let mut config = match &options.config {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        None if options.sets.is_empty() => return Ok(None),
        None => String::new(),
    };
    for (key, value) in &options.sets {
        let (section, name) = key
            .split_once('.')
            .ok_or_else(|| format!("--set expects <section>.<key>=<value>, got `{key}`"))?;
        config.push_str(&format!("\n[{section}]\n{name} = {value}\n"));
    }
    Ok(Some(config))
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--list-profiles" => {
                for profile in profiles::PROFILES {
                    println!("{:<22} {}", profile.device.id, profile.summary);
                }
                return Ok(None);
            }
            "--profile" => options.profile = Some(value("--profile")?),
            "--config" => options.config = Some(PathBuf::from(value("--config")?)),
            "--set" => {
                let set = value("--set")?;
                let (key, val) = set
                    .split_once('=')
                    .ok_or_else(|| format!("--set expects <key>=<value>, got `{set}`"))?;
                options.sets.push((key.to_string(), val.to_string()));
            }
            "--headless" => options.headless = true,
            "--script" => options.script = Some(value("--script")?),
            "--screenshot" => options.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--library" => options.library = Some(PathBuf::from(value("--library")?)),
            "--screenshot-dir" => {
                options.screenshot_dir = Some(PathBuf::from(value("--screenshot-dir")?));
            }
            "--trace-alloc" => {
                let bytes = value("--trace-alloc")?;
                budget::trace(
                    bytes
                        .parse()
                        .map_err(|_| format!("--trace-alloc expects bytes, got `{bytes}`"))?,
                );
            }
            "--heap-budget" => {
                let budget = value("--heap-budget")?;
                options.heap_budget = Some(if budget == "none" {
                    None
                } else {
                    let kib: usize = budget.parse().map_err(|_| {
                        format!("--heap-budget expects KiB or `none`, got `{budget}`")
                    })?;
                    Some(kib * 1024)
                });
            }
            other => return Err(format!("unknown argument `{other}` (see --help)")),
        }
    }
    Ok(Some(options))
}
