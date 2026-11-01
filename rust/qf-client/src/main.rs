//! Window/input lifecycle (winit). Wires input events to renderer and audio.
//! Renders a parsed BSP world mesh. Mouse look uses raw device motion (cursor events are ignored). Escape releases the grab, click or focus re-grabs, Shift+Escape or window close exits.

mod audio;
mod hud;
mod math;
mod render;
mod tone;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hud::HudElement;
use qf_engine::content::bsp::{Bsp, WorldMesh, read_pk3_entry};
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::entities::MapEntities;
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::recording::{MapIdentity, Recorder};
use qf_engine::simulation::walk::{ModeSimulation, MovementMode, WalkCommand};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const USAGE: &str = "usage: qf-client <map.bsp | pack.pk3> [--entry maps/wbomb1.bsp] [--sensitivity 3.0] [--record FILE]\nF9 starts/stops a movement recording (see docs/movement-recording.md); --record FILE starts one at launch.";

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
    audio: Option<audio::AudioOut>,
    world_mesh: Option<WorldMesh>,
    bsp: Bsp,
    simulation: ModeSimulation,
    /// Look direction owned by the client. The simulation only sees it on 125 Hz ticks, so
    /// re-reading it from there would drop input on frames that run no tick.
    view_angles: [f32; 3],
    pressed_keys: HashSet<KeyCode>,
    right_mouse_down: bool,
    hud_ups: hud::UpsCounter,
    hud_record: hud::RecordIndicator,
    recorder: Option<Recorder>,
    record_path: Option<PathBuf>,
    map: MapIdentity,
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
        bsp: Bsp,
        world_mesh: WorldMesh,
        player: NoclipPlayer,
        sensitivity: f32,
        map: MapIdentity,
        record_path: Option<PathBuf>,
    ) -> Self {
        let mut simulation = ModeSimulation::new(player);
        let entities = MapEntities::spawn(&bsp.parsed_entities, &bsp);
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
        let audio = match audio::AudioOut::new() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("audio unavailable: {e} (press B to retry)");
                None
            }
        };
        let mut app = Self {
            window: None,
            renderer: None,
            audio,
            world_mesh: Some(world_mesh),
            bsp,
            view_angles: simulation.mode().view().1,
            simulation,
            pressed_keys: HashSet::new(),
            right_mouse_down: false,
            hud_ups: hud::UpsCounter::new(),
            hud_record: hud::RecordIndicator::new(),
            recorder: None,
            record_path: None,
            map,
            hud_canvas: hud::Canvas::default(),
            sensitivity,
            mouse_grabbed: false,
            skip_next_motion: false,
            mouse_delta: [0.0; 2],
            last_frame: None,
        };
        if let Some(path) = record_path {
            app.start_recording(Some(path));
        }
        app
    }

    /// Starts recording walking ticks. Recording only copies each tick's command and result,
    /// so it cannot change the simulation.
    fn start_recording(&mut self, path: Option<PathBuf>) {
        let MovementMode::Walk(player) = self.simulation.mode() else {
            self.hud_record.show_message("REC NEED WALK");
            eprintln!("recording needs walk mode");
            return;
        };
        // Entities are installed, so live ticks take the trigger-aware path.
        let generator = concat!("qf-client ", env!("CARGO_PKG_VERSION"));
        self.recorder = Some(Recorder::start(
            self.map.clone(),
            generator,
            125,
            self.simulation.entities().is_some(),
            &player.state,
        ));
        self.record_path = Some(path.unwrap_or_else(default_recording_path));
        self.simulation.set_tick_logging(true);
        self.hud_record.set_recording(0);
        println!("recording started");
    }

    /// Drains logged ticks into the recorder; stops and saves if the player left walk mode.
    fn collect_recorded_ticks(&mut self) {
        let Some(recorder) = &mut self.recorder else {
            return;
        };
        for entry in self.simulation.take_tick_log() {
            recorder.push(&entry);
        }
        let ticks = recorder.tick_count();
        if self.simulation.mode().is_walking() {
            self.hud_record.set_recording(ticks);
        } else {
            self.stop_recording();
        }
    }

    fn stop_recording(&mut self) {
        self.simulation.set_tick_logging(false);
        let (Some(recorder), Some(path)) = (self.recorder.take(), self.record_path.take()) else {
            return;
        };
        let recording = recorder.finish();
        match std::fs::write(&path, recording.to_text()) {
            Ok(()) => {
                println!(
                    "saved recording: {} ticks -> {}",
                    recording.ticks.len(),
                    path.display()
                );
                self.hud_record.show_message("REC SAVED");
            }
            Err(e) => {
                eprintln!("failed to save recording {}: {e}", path.display());
                self.hud_record.show_message("REC FAILED");
            }
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
            match audio::AudioOut::new() {
                Ok(a) => self.audio = Some(a),
                Err(e) => eprintln!("cannot play sound: {e}"),
            }
        }
        if let Some(a) = &self.audio {
            a.play_beep();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("qfusion - WASD move, Space jump, C crouch, Right mouse/Shift dash, V walk/noclip, R/F noclip up/down, arrows look, B beep, +/- sensitivity, Esc release mouse, Shift+Esc quit");
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
        match render::Renderer::new(window.clone(), &world_mesh) {
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
                        Key::Named(NamedKey::F9) => {
                            if self.recorder.is_some() {
                                self.stop_recording();
                            } else {
                                self.start_recording(None);
                            }
                        }
                        Key::Character(ref key) if key.eq_ignore_ascii_case("b") => {
                            self.play_sound()
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
                if let Err(error) = self
                    .simulation
                    .advance_frame_with_entities(&world, &self.bsp, elapsed, command)
                {
                    eprintln!("player simulation failed: {error}");
                    event_loop.exit();
                    return;
                }
                self.collect_recorded_ticks();
                // Teleporters rewrite the look direction.
                self.view_angles = self.simulation.mode().view().1;
                for event in self.simulation.drain_trigger_events() {
                    println!("trigger: {event:?}");
                }
                if let Some(r) = &mut self.renderer {
                    let (eye, _) = self.simulation.mode().view();
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
                    self.hud_record.update(&hud::HudState {
                        velocity: [0.0; 3],
                        frame_seconds: elapsed.as_secs_f32(),
                    });
                    self.hud_ups.draw(&screen, &mut self.hud_canvas);
                    self.hud_record.draw(&screen, &mut self.hud_canvas);
                    r.render(eye, self.view_angles, &self.hud_canvas);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.stop_recording();
    }
}

fn default_recording_path() -> PathBuf {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    PathBuf::from(format!("qf-recording-{seconds}.qfrec"))
}

struct Options {
    bsp: Bsp,
    mesh: WorldMesh,
    player: NoclipPlayer,
    sensitivity: f32,
    map: MapIdentity,
    record_path: Option<PathBuf>,
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
    let mut record_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--entry" => entry = args.next().ok_or("--entry requires a value")?,
            "--sensitivity" => {
                sensitivity = args
                    .next()
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .ok_or("--sensitivity requires a positive number")?
            }
            "--record" => {
                record_path = Some(PathBuf::from(
                    args.next().ok_or("--record requires a file")?,
                ))
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
    let is_pk3 = input.to_lowercase().ends_with(".pk3");
    let (map_entry, map_name) = if is_pk3 {
        let stem = Path::new(&entry).file_stem().and_then(|s| s.to_str());
        (entry.clone(), stem.unwrap_or("map").to_string())
    } else {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("map");
        (format!("maps/{stem}.bsp"), stem.to_string())
    };
    let map = MapIdentity::of_bsp(&map_name, &map_entry, &data);
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
    for diagnostic in &bsp.diagnostics {
        eprintln!("BSP: {diagnostic}");
    }
    Ok(Some(Options {
        bsp,
        mesh,
        player,
        sensitivity,
        map,
        record_path,
    }))
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
        options.mesh,
        options.player,
        options.sensitivity,
        options.map,
        options.record_path,
    )) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
