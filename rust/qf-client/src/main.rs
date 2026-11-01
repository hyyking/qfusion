//! Window/input lifecycle (winit). Wires input events to renderer and audio.
//! Renders a parsed BSP world mesh. Mouse look uses raw device motion (cursor events are ignored). Escape releases the grab, click or focus re-grabs, Shift+Escape or window close exits.

mod bomb;
mod game_sound;
mod headless;
mod hud;
mod math;
mod render;
mod world;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hud::HudElement;
use qf_engine::content::bsp::{Bsp, WorldMesh, read_pk3_entry};
use qf_engine::content::shader::ShaderLib;
use qf_engine::game::Team;
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::entities::{MapEntities, TriggerEvent};
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::walk::{ModeSimulation, WalkCommand};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const USAGE: &str = "usage: qf-client <map.bsp | pack.pk3> [--entry maps/wbomb1.bsp] [--sensitivity 3.0] [--game-dir <basewsw>]... [--team alpha|beta] [--freeplay] [--screenshot out.png [--shot-eye x,y,z] [--shot-angles p,y,r] [--shot-size WxH]]";

/// Legacy defaults (`source/client/cgame/cg_input.cpp`): view delta is
/// counts * sensitivity * m_yaw/m_pitch with m_yaw = m_pitch = 0.022; acceleration
/// (`m_accel`) and `m_filter` default to off and are not implemented.
const DEGREES_PER_COUNT: f32 = 0.022;
const SENSITIVITY_STEP: f32 = 0.5;
const MIN_SENSITIVITY: f32 = 0.5;
const DEFAULT_SENSITIVITY: f32 = 3.0;
const MAX_PITCH: f32 = 89.0;

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    audio: Option<game_sound::GameSound>,
    game_dirs: Vec<PathBuf>,
    map_pk3: Option<PathBuf>,
    world_mesh: Option<WorldMesh>,
    bsp: Arc<Bsp>,
    /// Local Bomb and Defuse match; absent with `--freeplay`.
    bomb: Option<bomb::BombSession>,
    simulation: ModeSimulation,
    /// Look direction owned by the client. The simulation only sees it on 125 Hz ticks, so
    /// re-reading it from there would drop input on frames that run no tick.
    view_angles: [f32; 3],
    pressed_keys: HashSet<KeyCode>,
    right_mouse_down: bool,
    hud_ups: hud::UpsCounter,
    hud_canvas: hud::Canvas,
    sensitivity: f32,
    mouse_grabbed: bool,
    /// The first raw motion after a grab can carry a stale jump; drop it.
    skip_next_motion: bool,
    /// Raw motion accumulated since the last frame; consumed once per redraw.
    mouse_delta: [f64; 2],
    last_frame: Option<Instant>,
}

impl App {
    fn new(
        bsp: Arc<Bsp>,
        bomb: Option<bomb::BombSession>,
        world_mesh: WorldMesh,
        player: NoclipPlayer,
        sensitivity: f32,
        game_dirs: Vec<PathBuf>,
        map_pk3: Option<PathBuf>,
    ) -> Self {
        let mut simulation = ModeSimulation::new(player);
        let entities = MapEntities::spawn(&bsp.parsed_entities, &*bsp);
        for warning in entities.warnings() {
            eprintln!(
                "entity {} ({}): {}",
                warning.entity, warning.classname, warning.reason
            );
        }
        println!(
            "map entities: {} triggers simulated, {} classnames unsupported",
            entities.trigger_count(),
            entities.unsupported_classnames().len()
        );
        simulation.set_entities(entities);
        simulation.toggle_mode();
        let mut app = Self {
            window: None,
            renderer: None,
            audio: None,
            game_dirs,
            map_pk3,
            world_mesh: Some(world_mesh),
            bsp,
            bomb,
            view_angles: simulation.mode().view().1,
            simulation,
            pressed_keys: HashSet::new(),
            right_mouse_down: false,
            hud_ups: hud::UpsCounter::new(),
            hud_canvas: hud::Canvas::default(),
            sensitivity,
            mouse_grabbed: false,
            skip_next_motion: false,
            mouse_delta: [0.0; 2],
            last_frame: None,
        };
        app.start_audio();
        app
    }

    fn start_audio(&mut self) {
        let vfs = game_sound::mount_content(&self.game_dirs, self.map_pk3.as_deref());
        match game_sound::GameSound::new(vfs, &self.bsp) {
            Ok(a) => self.audio = Some(a),
            Err(e) => eprintln!("audio unavailable: {e} (press B to retry)"),
        }
    }

    fn key_axis(&self, positive: KeyCode, negative: KeyCode) -> f32 {
        let positive = if self.pressed_keys.contains(&positive) {
            1.0
        } else {
            0.0
        };
        let negative = if self.pressed_keys.contains(&negative) {
            1.0
        } else {
            0.0
        };
        positive - negative
    }

    fn set_mouse_grab(&mut self, grab: bool) {
        let Some(window) = &self.window else { return };
        if grab {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Confined)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked))
                .is_ok();
            self.skip_next_motion = grabbed;
            window.set_cursor_visible(!grabbed);
            self.mouse_grabbed = grabbed;
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.mouse_grabbed = false;
        }
        self.mouse_delta = [0.0; 2];
    }

    fn play_sound(&mut self) {
        if self.audio.is_none() {
            self.start_audio();
        }
        if let Some(a) = &mut self.audio {
            a.play_test_sound();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("qfusion - WASD move, Space jump, C crouch, Right mouse/Shift dash, V walk/noclip, R/F noclip up/down, arrows look, B test sound, +/- sensitivity, Esc release mouse, Shift+Esc quit");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let Some(world_mesh) = self.world_mesh.take() else {
            eprintln!("failed to initialize renderer: parsed BSP mesh is unavailable");
            event_loop.exit();
            return;
        };
        let mut submodels: Vec<(usize, WorldMesh)> = Vec::new();
        for pose in self
            .simulation
            .entities()
            .map(|e| e.mover_poses())
            .unwrap_or_default()
        {
            if submodels.iter().all(|(m, _)| *m != pose.model) {
                match self.bsp.model_mesh(pose.model) {
                    Ok(mesh) => submodels.push((pose.model, mesh)),
                    Err(e) => eprintln!("mover model *{}: {e}", pose.model),
                }
            }
        }
        let content = load_content(&self.game_dirs, self.map_pk3.as_deref());
        match render::Renderer::new(window.clone(), &content, &self.bsp, &world_mesh, submodels) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                eprintln!("failed to initialize renderer: {e}");
                event_loop.exit();
                return;
            }
        }
        window.request_redraw();
        self.window = Some(window);
        self.set_mouse_grab(true);
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.mouse_grabbed
        {
            if std::mem::take(&mut self.skip_next_motion) {
                return;
            }
            self.mouse_delta[0] += delta.0;
            self.mouse_delta[1] += delta.1;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        self.pressed_keys.insert(code);
                    } else {
                        self.pressed_keys.remove(&code);
                    }
                }
                if event.state == ElementState::Pressed && !event.repeat {
                    match event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            let shift = self.pressed_keys.contains(&KeyCode::ShiftLeft)
                                || self.pressed_keys.contains(&KeyCode::ShiftRight);
                            if shift {
                                event_loop.exit();
                            } else {
                                self.set_mouse_grab(false);
                            }
                        }
                        Key::Character(ref key) if key == "+" || key == "=" => {
                            self.sensitivity += SENSITIVITY_STEP;
                            println!("sensitivity {}", self.sensitivity);
                        }
                        Key::Character(ref key) if key == "-" || key == "_" => {
                            self.sensitivity =
                                (self.sensitivity - SENSITIVITY_STEP).max(MIN_SENSITIVITY);
                            println!("sensitivity {}", self.sensitivity);
                        }
                        Key::Character(ref key) if key.eq_ignore_ascii_case("b") => {
                            self.play_sound()
                        }
                        Key::Character(ref key) if key.eq_ignore_ascii_case("g") => {
                            if let Some(bomb) = &mut self.bomb {
                                bomb.command("drop", "");
                            }
                        }
                        Key::Character(ref key) if key.eq_ignore_ascii_case("k") => {
                            if let Some(bomb) = &mut self.bomb {
                                bomb.command("suicide", "");
                            }
                        }
                        Key::Character(ref key) if key.eq_ignore_ascii_case("t") => {
                            if let Some(bomb) = &mut self.bomb {
                                bomb.switch_team();
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => self.right_mouse_down = state == ElementState::Pressed,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !self.mouse_grabbed => self.set_mouse_grab(true),
            WindowEvent::Focused(false) => {
                self.pressed_keys.clear();
                self.right_mouse_down = false;
                self.set_mouse_grab(false);
            }
            WindowEvent::Focused(true) => self.set_mouse_grab(true),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let elapsed = self
                    .last_frame
                    .replace(now)
                    .map_or(Duration::ZERO, |last| now.duration_since(last));
                let look_speed = 90.0 * elapsed.as_secs_f32();
                let mut view_angles = self.view_angles;
                view_angles[0] += self.key_axis(KeyCode::ArrowUp, KeyCode::ArrowDown) * look_speed;
                view_angles[1] +=
                    self.key_axis(KeyCode::ArrowRight, KeyCode::ArrowLeft) * look_speed;
                let [dx, dy] = std::mem::take(&mut self.mouse_delta);
                let degrees = DEGREES_PER_COUNT * self.sensitivity;
                view_angles[1] -= dx as f32 * degrees;
                view_angles[0] =
                    (view_angles[0] - dy as f32 * degrees).clamp(-MAX_PITCH, MAX_PITCH);
                let up = if self.simulation.mode().is_walking() {
                    self.key_axis(KeyCode::Space, KeyCode::KeyC)
                } else {
                    self.key_axis(KeyCode::KeyR, KeyCode::KeyF)
                };
                let command = WalkCommand {
                    forward: self.key_axis(KeyCode::KeyW, KeyCode::KeyS),
                    right: self.key_axis(KeyCode::KeyD, KeyCode::KeyA),
                    up,
                    special: self.right_mouse_down
                        || self.pressed_keys.contains(&KeyCode::ShiftLeft),
                    view_angles,
                };
                self.view_angles = view_angles;
                let world = BspWorld::new(&self.bsp);
                if let Some(bomb) = &mut self.bomb {
                    bomb.apply_movement_lock(&mut self.simulation);
                }
                let ticks = match self
                    .simulation
                    .advance_frame_with_entities(&world, &*self.bsp, elapsed, command)
                {
                    Ok(ticks) => ticks,
                    Err(error) => {
                        eprintln!("player simulation failed: {error}");
                        event_loop.exit();
                        return;
                    }
                };
                let mut respawned = false;
                if let Some(bomb) = &mut self.bomb {
                    match bomb.advance(ticks, &mut self.simulation) {
                        Ok(spawned) => respawned = spawned,
                        Err(error) => {
                            eprintln!("{error}");
                            event_loop.exit();
                            return;
                        }
                    }
                }
                let trigger_events = self.simulation.drain_trigger_events();
                let teleported = trigger_events
                    .iter()
                    .any(|event| matches!(event, TriggerEvent::Teleport { .. }));
                self.view_angles = view_after_frame(
                    self.view_angles,
                    self.simulation.mode().view().1,
                    teleported || respawned,
                );
                let (eye, _) = self.simulation.mode().view();
                for event in &trigger_events {
                    println!("trigger: {event:?}");
                }
                if let Some(audio) = &mut self.audio {
                    audio.update(
                        eye,
                        self.view_angles[1],
                        &mut self.simulation,
                        &trigger_events,
                    );
                }
                if let Some(r) = &mut self.renderer {
                    let [width, height] = r.size();
                    let scale = self
                        .window
                        .as_ref()
                        .map_or(1.0, |w| w.scale_factor() as f32);
                    let screen = hud::Screen {
                        width,
                        height,
                        scale,
                    };
                    self.hud_ups.update(&hud::HudState {
                        velocity: self.simulation.mode().velocity(),
                        frame_seconds: elapsed.as_secs_f32(),
                    });
                    self.hud_canvas.clear();
                    self.hud_ups.draw(&screen, &mut self.hud_canvas);
                    if let Some(bomb) = &self.bomb {
                        bomb.hud.draw(&screen, &mut self.hud_canvas);
                    }
                    let movers = self
                        .simulation
                        .entities()
                        .map(|e| e.mover_poses())
                        .unwrap_or_default();
                    r.render(eye, self.view_angles, &movers, &self.hud_canvas);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// The simulation only stores view angles when a 125 Hz tick runs, so adopting them on every
/// frame would discard the frame's mouse/arrow delta whenever no tick ran. Only teleporters
/// and gametype spawns, which rewrite the look direction inside the simulation, override the
/// client-owned angles.
fn view_after_frame(client: [f32; 3], simulation: [f32; 3], rewritten: bool) -> [f32; 3] {
    if rewritten { simulation } else { client }
}

struct Screenshot {
    path: PathBuf,
    eye: Option<[f32; 3]>,
    angles: Option<[f32; 3]>,
    size: (u32, u32),
}

struct Options {
    bsp: Arc<Bsp>,
    bomb: Option<bomb::BombSession>,
    mesh: WorldMesh,
    player: NoclipPlayer,
    sensitivity: f32,
    game_dirs: Vec<PathBuf>,
    map_pk3: Option<PathBuf>,
    screenshot: Option<Screenshot>,
}

fn parse_triple(value: Option<String>, flag: &str) -> Result<[f32; 3], String> {
    let parsed: Vec<f32> = value
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map_while(|v| v.trim().parse().ok())
        .collect();
    parsed
        .try_into()
        .map_err(|_| format!("{flag} requires three comma-separated numbers"))
}

fn load_content(game_dirs: &[PathBuf], map_pk3: Option<&Path>) -> world::Content {
    let mut shaders = ShaderLib::new();
    let vfs = game_sound::mount_content(game_dirs, map_pk3);
    let scripts = shaders.load_vfs(&vfs);
    println!(
        "content: {scripts} shaders parsed ({} script warnings)",
        shaders.warnings.len()
    );
    if std::env::var_os("QF_SHADER_WARNINGS").is_some() {
        for w in &shaders.warnings {
            eprintln!("shader: {w}");
        }
    }
    world::Content { vfs, shaders }
}

fn load_world_mesh() -> Result<Option<Options>, String> {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        return Err("missing map input".into());
    };
    if input == "--help" || input == "-h" {
        println!("{USAGE}");
        return Ok(None);
    }

    let mut entry = String::from("maps/wbomb1.bsp");
    let mut sensitivity = DEFAULT_SENSITIVITY;
    let mut game_dirs = Vec::new();
    let mut team = Team::Alpha;
    let mut freeplay = false;
    let mut screenshot: Option<Screenshot> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--screenshot" => {
                screenshot = Some(Screenshot {
                    path: PathBuf::from(args.next().ok_or("--screenshot requires a path")?),
                    eye: None,
                    angles: None,
                    size: (1280, 720),
                });
            }
            "--shot-eye" | "--shot-angles" | "--shot-size" => {
                let shot = screenshot
                    .as_mut()
                    .ok_or_else(|| format!("{arg} must follow --screenshot"))?;
                match arg.as_str() {
                    "--shot-eye" => shot.eye = Some(parse_triple(args.next(), &arg)?),
                    "--shot-angles" => shot.angles = Some(parse_triple(args.next(), &arg)?),
                    _ => {
                        shot.size = args
                            .next()
                            .and_then(|v| {
                                let (w, h) = v.split_once('x')?;
                                Some((w.parse().ok()?, h.parse().ok()?))
                            })
                            .filter(|&(w, h)| w > 0 && h > 0)
                            .ok_or("--shot-size requires WIDTHxHEIGHT")?
                    }
                }
            }
            "--freeplay" => freeplay = true,
            "--team" => {
                team = match args.next().as_deref() {
                    Some("alpha") => Team::Alpha,
                    Some("beta") => Team::Beta,
                    _ => return Err("--team requires alpha or beta".into()),
                }
            }
            "--entry" => entry = args.next().ok_or("--entry requires a value")?,
            "--game-dir" => {
                game_dirs.push(PathBuf::from(
                    args.next().ok_or("--game-dir requires a value")?,
                ));
            }
            "--sensitivity" => {
                sensitivity = args
                    .next()
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .ok_or("--sensitivity requires a positive number")?
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }

    let path = Path::new(&input);
    let data = if input.to_lowercase().ends_with(".pk3") {
        read_pk3_entry(path, &entry)?
    } else {
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
    };
    let bsp = Bsp::parse(&data).map_err(|e| e.to_string())?;
    let player = NoclipPlayer::spawn(&bsp.parsed_entities).map_err(|e| e.to_string())?;
    let mesh = bsp.model_mesh(0).map_err(|e| e.to_string())?;

    println!(
        "loaded {input}: {} vertices, {} triangles, {} faces skipped",
        mesh.positions.len(),
        mesh.indices.len() / 3,
        mesh.skipped_faces
    );
    if let Some((mins, maxs)) = mesh.bounds() {
        println!("world bounds: {mins:?} .. {maxs:?}");
    }
    let bsp = Arc::new(bsp);
    let bomb = if freeplay {
        None
    } else {
        Some(bomb::BombSession::new(bsp.clone(), team, 1).map_err(|e| e.to_string())?)
    };
    for diagnostic in &bsp.diagnostics {
        eprintln!("BSP: {diagnostic}");
    }
    Ok(Some(Options {
        bsp,
        bomb,
        mesh,
        player,
        sensitivity,
        game_dirs,
        map_pk3: input
            .to_lowercase()
            .ends_with(".pk3")
            .then(|| path.to_path_buf()),
        screenshot,
    }))
}

fn take_screenshot(options: &Options, shot: &Screenshot) -> Result<(), Box<dyn std::error::Error>> {
    let content = load_content(&options.game_dirs, options.map_pk3.as_deref());
    let (eye, angles) = ModeSimulation::new(options.player.clone()).mode().view();
    headless::screenshot(
        &shot.path,
        &content,
        &options.bsp,
        &options.mesh,
        shot.eye.unwrap_or(eye),
        shot.angles.unwrap_or(angles),
        shot.size,
    )
}

fn main() {
    let options = match load_world_mesh() {
        Ok(Some(map)) => map,
        Ok(None) => return,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            std::process::exit(1);
        }
    };
    if let Some(shot) = &options.screenshot {
        if let Err(e) = take_screenshot(&options, shot) {
            eprintln!("screenshot failed: {e}");
            std::process::exit(1);
        }
        println!("wrote {}", shot.path.display());
        return;
    }

    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to create event loop: {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut App::new(
        options.bsp,
        options.bomb,
        options.mesh,
        options.player,
        options.sensitivity,
        options.game_dirs,
        options.map_pk3,
    )) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_view_delta_survives_frames_without_ticks() {
        let stale_simulation = [0.0, 10.0, 0.0];
        let mut view = stale_simulation;
        for _ in 0..100 {
            // Each frame adds a small mouse delta; no tick ran, so the simulation is stale.
            view[1] -= 0.5;
            view = view_after_frame(view, stale_simulation, false);
        }
        assert_eq!(view[1], 10.0 - 50.0);
    }

    #[test]
    fn teleport_or_spawn_overrides_client_view() {
        assert_eq!(
            view_after_frame([0.0, 1.0, 0.0], [0.0, 90.0, 0.0], true),
            [0.0, 90.0, 0.0]
        );
    }
}
