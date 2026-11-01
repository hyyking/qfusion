//! Window/input lifecycle (winit). Wires input events to renderer and audio.
//! Space plays a beep, Escape or window close exits.

mod audio;
mod cube;
mod math;
mod render;
mod tone;

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    audio: Option<audio::AudioOut>,
    start: Instant,
}

impl App {
    fn new() -> Self {
        let audio = match audio::AudioOut::new() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("audio unavailable: {e} (press Space to retry)");
                None
            }
        };
        Self {
            window: None,
            renderer: None,
            audio,
            start: Instant::now(),
        }
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
            .with_title("qfusion platform POC - Space: beep, Esc: quit");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        match render::Renderer::new(window.clone()) {
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
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::Space) => self.play_sound(),
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(r) = &mut self.renderer {
                    r.render(self.start.elapsed().as_secs_f32());
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to create event loop: {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut App::new()) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
