//! Window/input lifecycle (winit). Wires input events to renderer and audio.
//! Renders a parsed BSP world mesh. Space plays a beep, Escape or window close exits.

mod audio;
mod math;
mod render;
mod tone;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use qf_engine::content::bsp::{Bsp, WorldMesh, read_pk3_entry};
use qf_engine::simulation::player::{FixedStepSimulation, NoclipPlayer, PlayerCommand};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

const USAGE: &str = "usage: qf-client <map.bsp | pack.pk3> [--entry maps/wbomb1.bsp]";

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    audio: Option<audio::AudioOut>,
    world_mesh: Option<WorldMesh>,
    simulation: FixedStepSimulation,
    pressed_keys: HashSet<KeyCode>,
    last_frame: Option<Instant>,
}

impl App {
    fn new(world_mesh: WorldMesh, player: NoclipPlayer) -> Self {
        let audio = match audio::AudioOut::new() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("audio unavailable: {e} (press B to retry)");
                None
            }
        };
        Self {
            window: None,
            renderer: None,
            audio,
            world_mesh: Some(world_mesh),
            simulation: FixedStepSimulation::new(player),
            pressed_keys: HashSet::new(),
            last_frame: None,
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
            .with_title("qfusion noclip - WASD move, R/F rise/fall, arrows look, B beep, Esc quit");
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
                        Key::Named(NamedKey::Escape) => event_loop.exit(),
                        Key::Character(ref key) if key.eq_ignore_ascii_case("b") => {
                            self.play_sound()
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::Focused(false) => self.pressed_keys.clear(),
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
                let mut view_angles = self.simulation.player().view_angles;
                view_angles[0] += self.key_axis(KeyCode::ArrowUp, KeyCode::ArrowDown) * look_speed;
                view_angles[1] +=
                    self.key_axis(KeyCode::ArrowRight, KeyCode::ArrowLeft) * look_speed;
                let command = PlayerCommand {
                    forward: self.key_axis(KeyCode::KeyW, KeyCode::KeyS),
                    right: self.key_axis(KeyCode::KeyD, KeyCode::KeyA),
                    up: self.key_axis(KeyCode::KeyR, KeyCode::KeyF),
                    view_angles,
                };
                if let Err(error) = self.simulation.advance_frame(elapsed, command) {
                    eprintln!("player simulation failed: {error}");
                    event_loop.exit();
                    return;
                }
                if let Some(r) = &mut self.renderer {
                    r.render(self.simulation.player());
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn load_world_mesh() -> Result<Option<(WorldMesh, NoclipPlayer)>, String> {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        return Err("missing map input".into());
    };
    if input == "--help" || input == "-h" {
        println!("{USAGE}");
        return Ok(None);
    }

    let mut entry = String::from("maps/wbomb1.bsp");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--entry" => entry = args.next().ok_or("--entry requires a value")?,
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
    for diagnostic in &bsp.diagnostics {
        eprintln!("BSP: {diagnostic}");
    }
    Ok(Some((mesh, player)))
}

fn main() {
    let (world_mesh, player) = match load_world_mesh() {
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
    if let Err(e) = event_loop.run_app(&mut App::new(world_mesh, player)) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
