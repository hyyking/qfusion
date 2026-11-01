//! Replays movement recordings and compares them with the legacy `gs_pmove.cpp` reference.
//!
//! ```text
//! qf-movecmp sample  --map <pk3|bsp> [--entry maps/wbomb1.bsp] --out sample.qfrec
//! qf-movecmp compare --recording run.qfrec [--map <pk3|bsp>] [--entry E]
//!                    (--legacy-exe pmove_replay | --legacy-recording legacy.qfrec
//!                       [--legacy-trace trace.txt] | --rust-only)
//!                    [--out-dir DIR] [--tol-position 0.05] [--tol-velocity 0.05] [--window 125]
//! ```
//!
//! Exit status: 0 identical within tolerance, 1 divergence, 2 usage or I/O error.

use std::path::{Path, PathBuf};
use std::process::Command;

use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::compare::{
    Cause, LoggingWorld, Report, Tolerances, classify, compare, first_exact_mismatch,
    parse_trace_log, trace_log_to_text,
};
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::pmove::PmoveRules;
use qf_engine::simulation::recording::{
    MapIdentity, Recording, TickResult, record_script, replay, replay_with, sample_script,
};

const USAGE: &str = "usage:
  qf-movecmp sample  --map <pk3|bsp> [--entry maps/wbomb1.bsp] [--yaw DEG] --out FILE
  qf-movecmp compare --recording FILE [--map <pk3|bsp>] [--entry ENTRY]
                     (--legacy-exe pmove_replay | --legacy-recording FILE [--legacy-trace FILE] | --rust-only)
                     [--out-dir DIR] [--per-step] [--tol-position N] [--tol-velocity N] [--window TICKS]

Without --map the map pack is looked up as map_<name>.pk3 in $WARSOW_BASEWSW (default
C:\\Program Files (x86)\\Warsow 2.1\\basewsw). Exit status: 0 match, 1 divergence, 2 error.";

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

#[derive(Default)]
struct Args {
    map: Option<PathBuf>,
    entry: Option<String>,
    out: Option<PathBuf>,
    recording: Option<PathBuf>,
    legacy_exe: Option<PathBuf>,
    legacy_recording: Option<PathBuf>,
    legacy_trace: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    rust_only: bool,
    per_step: bool,
    yaw: f32,
    window: usize,
    tolerances: Tolerances,
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        window: 125,
        tolerances: Tolerances::default(),
        ..Args::default()
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        let number = |s: String| {
            s.parse::<f32>()
                .map_err(|_| format!("invalid number {s:?}"))
        };
        match arg.as_str() {
            "--map" => a.map = Some(value("--map")?.into()),
            "--entry" => a.entry = Some(value("--entry")?),
            "--out" => a.out = Some(value("--out")?.into()),
            "--recording" => a.recording = Some(value("--recording")?.into()),
            "--legacy-exe" => a.legacy_exe = Some(value("--legacy-exe")?.into()),
            "--legacy-recording" => a.legacy_recording = Some(value("--legacy-recording")?.into()),
            "--legacy-trace" => a.legacy_trace = Some(value("--legacy-trace")?.into()),
            "--out-dir" => a.out_dir = Some(value("--out-dir")?.into()),
            "--rust-only" => a.rust_only = true,
            "--per-step" => a.per_step = true,
            "--yaw" => a.yaw = number(value("--yaw")?)?,
            "--window" => {
                a.window = value("--window")?
                    .parse()
                    .map_err(|_| "invalid --window".to_string())?;
            }
            "--tol-position" => a.tolerances.position = number(value("--tol-position")?)?,
            "--tol-velocity" => a.tolerances.velocity = number(value("--tol-velocity")?)?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn run() -> Result<i32, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("missing command")?;
    let args = parse_args(args)?;
    match command.as_str() {
        "sample" => sample(&args),
        "compare" => compare_command(&args),
        _ => Err(format!("unknown command {command}")),
    }
}

fn default_pk3(map_name: &str) -> PathBuf {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    Path::new(&base).join(format!("map_{map_name}.pk3"))
}

fn load_bsp(path: &Path, entry: &str) -> Result<Vec<u8>, String> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pk3"))
    {
        read_pk3_entry(path, entry).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn sample(args: &Args) -> Result<i32, String> {
    let map = args.map.as_ref().ok_or("--map is required")?;
    let out = args.out.as_ref().ok_or("--out is required")?;
    let entry = args
        .entry
        .clone()
        .unwrap_or_else(|| "maps/wbomb1.bsp".into());
    let data = load_bsp(map, &entry)?;
    let bsp = Bsp::parse(&data).map_err(|e| e.to_string())?;
    let name = Path::new(&entry)
        .file_stem()
        .map_or_else(|| entry.clone(), |s| s.to_string_lossy().into_owned());
    let mut spawn = NoclipPlayer::spawn(&bsp.parsed_entities)
        .map_err(|e| format!("map has no usable spawn point: {e:?}"))?;
    spawn.view_angles = [0.0, args.yaw, 0.0];
    let world = BspWorld::new(&bsp);
    let recording = record_script(
        &world,
        spawn,
        MapIdentity::of_bsp(&name, &entry, &data),
        &format!("qf-movecmp {} scripted sample", env!("CARGO_PKG_VERSION")),
        &sample_script(args.yaw),
    )
    .map_err(|e| format!("{e:?}"))?;
    std::fs::write(out, recording.to_text()).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {} ticks to {}", recording.ticks.len(), out.display());
    Ok(0)
}

fn results(recording: &Recording) -> Vec<TickResult> {
    recording.ticks.iter().map(|t| t.result.clone()).collect()
}

fn run_legacy(
    exe: &Path,
    recording: &Path,
    bsp: &Path,
    out: &Path,
    trace: Option<(usize, &Path)>,
    per_step: bool,
) -> Result<(), String> {
    let mut command = Command::new(exe);
    if per_step {
        command.arg("--per-step");
    }
    command
        .arg("--recording")
        .arg(recording)
        .arg("--bsp")
        .arg(bsp)
        .arg("--out")
        .arg(out);
    if let Some((tick, path)) = trace {
        command
            .arg("--trace-from")
            .arg(tick.to_string())
            .arg("--trace-to")
            .arg(tick.to_string())
            .arg("--trace-out")
            .arg(path);
    }
    let output = command
        .output()
        .map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed ({}): {}",
            exe.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn compare_command(args: &Args) -> Result<i32, String> {
    let recording_path = args.recording.as_ref().ok_or("--recording is required")?;
    let text = std::fs::read_to_string(recording_path)
        .map_err(|e| format!("{}: {e}", recording_path.display()))?;
    let recording = Recording::from_text(&text).map_err(|e| e.to_string())?;
    let entry = args
        .entry
        .clone()
        .unwrap_or_else(|| recording.map.entry.clone());
    let map_path = args
        .map
        .clone()
        .unwrap_or_else(|| default_pk3(&recording.map.name));
    let data = load_bsp(&map_path, &entry)?;
    if !recording.map.matches(&data) {
        return Err(format!(
            "{} is not the map the recording was made on (recorded {} bytes, fnv1a64 {:016x}; found {} bytes, fnv1a64 {:016x})",
            map_path.display(),
            recording.map.size,
            recording.map.fnv1a64,
            data.len(),
            qf_engine::simulation::recording::fnv1a64(&data)
        ));
    }
    let bsp = Bsp::parse(&data).map_err(|e| e.to_string())?;
    let world = BspWorld::new(&bsp);
    let rules = PmoveRules::default();

    let rust = replay(&world, &rules, &recording);
    let recorded = results(&recording);
    let replay_mismatch = first_exact_mismatch(&rust, &recorded);
    let trigger_ticks = recording.ticks.iter().filter(|t| t.result.trigger).count();

    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("movecmp-report"));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    if args.rust_only {
        return rust_only(
            args,
            &recording,
            &rust,
            replay_mismatch,
            trigger_ticks,
            &out_dir,
        );
    }

    let (legacy_recording, legacy_description) = if let Some(exe) = &args.legacy_exe {
        let bsp_path = out_dir.join("map.bsp");
        std::fs::write(&bsp_path, &data).map_err(|e| e.to_string())?;
        let out = out_dir.join("legacy.qfrec");
        run_legacy(exe, recording_path, &bsp_path, &out, None, args.per_step)?;
        let description = format!("legacy `gs_pmove.cpp` via `{}`", exe.display());
        (read_recording(&out)?, description)
    } else if let Some(path) = &args.legacy_recording {
        (
            read_recording(path)?,
            format!("legacy output `{}`", path.display()),
        )
    } else {
        return Err("one of --legacy-exe, --legacy-recording or --rust-only is required".into());
    };
    let mut legacy = results(&legacy_recording);
    // Entity-driven events and touches do not exist in the legacy movement code.
    for (legacy, recorded) in legacy.iter_mut().zip(&recording.ticks) {
        if recorded.result.trigger {
            legacy.events = recorded.result.events.clone();
            legacy.touches = recorded.result.touches.clone();
        }
    }
    let comparison = compare(&rust, &legacy, &args.tolerances);

    let mut cause = None;
    if let Some(first) = comparison.first_divergence() {
        let tick = first.tick;
        let legacy_trace = if let Some(exe) = &args.legacy_exe {
            let path = out_dir.join("legacy-trace.txt");
            let scratch = out_dir.join("legacy-trace-run.qfrec");
            run_legacy(
                exe,
                recording_path,
                &out_dir.join("map.bsp"),
                &scratch,
                Some((tick, &path)),
                args.per_step,
            )?;
            let _ = std::fs::remove_file(scratch);
            Some(path)
        } else {
            args.legacy_trace.clone()
        };
        if let Some(path) = legacy_trace {
            let log =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let legacy_calls: Vec<_> = parse_trace_log(&log)?
                .into_iter()
                .filter(|(t, _)| *t == tick)
                .map(|(_, call)| call)
                .collect();
            let logging = LoggingWorld::new(&world, tick, tick);
            replay_with(&logging, &rules, &recording, |t| logging.set_tick(t));
            let rust_log = logging.into_log();
            std::fs::write(out_dir.join("rust-trace.txt"), trace_log_to_text(&rust_log))
                .map_err(|e| e.to_string())?;
            let rust_calls: Vec<_> = rust_log.into_iter().map(|(_, call)| call).collect();
            cause = Some(classify(
                &rust_calls,
                &legacy_calls,
                first,
                &args.tolerances,
            ));
        }
    }

    write_report(
        args,
        &out_dir,
        &Report {
            recording: &recording,
            recording_name: &recording_path.display().to_string(),
            legacy_description: &legacy_description,
            rust: &rust,
            legacy: &legacy,
            comparison: &comparison,
            tolerances: &args.tolerances,
            cause: cause.as_ref(),
            replay_mismatch,
            trigger_ticks,
            window: args.window,
        },
    )
}

fn read_recording(path: &Path) -> Result<Recording, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Recording::from_text(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_report(args: &Args, out_dir: &Path, report: &Report) -> Result<i32, String> {
    let _ = args;
    std::fs::write(out_dir.join("report.md"), report.to_markdown()).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("comparison.csv"), report.to_csv()).map_err(|e| e.to_string())?;
    let c = report.comparison;
    println!(
        "{}: {} ticks, {} outside tolerance, max position error {:.4}, max velocity error {:.4}",
        if report.passed() { "PASS" } else { "FAIL" },
        c.diffs.len(),
        c.diverged_ticks(),
        c.max_position_error(),
        c.max_velocity_error()
    );
    if let Some(first) = c.first_divergence() {
        println!("first divergence at tick {}", first.tick);
    }
    if let Some(cause) = report.cause {
        println!("cause: {}: {cause}", cause.kind());
    } else if c.first_divergence().is_some() {
        println!("cause: {}", Cause::Unknown);
    }
    if let Some(tick) = report.replay_mismatch {
        println!("warning: Rust replay differs from the recorded Rust state at tick {tick}");
    }
    println!("report: {}", out_dir.join("report.md").display());
    Ok(i32::from(!report.passed()))
}

fn rust_only(
    args: &Args,
    recording: &Recording,
    rust: &[TickResult],
    replay_mismatch: Option<usize>,
    trigger_ticks: usize,
    out_dir: &Path,
) -> Result<i32, String> {
    let recorded = results(recording);
    let comparison = compare(rust, &recorded, &args.tolerances);
    write_report(
        args,
        out_dir,
        &Report {
            recording,
            recording_name: "(rust replay only)",
            legacy_description: "the Rust results stored in the recording",
            rust,
            legacy: &recorded,
            comparison: &comparison,
            tolerances: &args.tolerances,
            cause: None,
            replay_mismatch,
            trigger_ticks,
            window: args.window,
        },
    )
}
