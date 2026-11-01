# qf-platform-poc

Small Rust proof of concept for the desktop platform/runtime foundation of a qfusion port:
**winit** (window/input) + **wgpu** (rendering) + **cpal** (audio). No Bevy, no gameplay, no BSP, no networking.
It is a standalone Cargo project (its own `[workspace]`); it does not touch the C/C++ build.

## Build / run

Requires a stable Rust toolchain (developed with 1.97.1, edition 2024) and a GPU driver supporting Vulkan, DX12, Metal or GL.

```
cd rust/qf-platform-poc
cargo run --release
```

Controls: **Space** plays a 0.3 s 660 Hz beep, **Esc** or closing the window exits.

Checks: `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test` (tests need no GPU/audio device).

Linux needs ALSA dev headers for cpal (`libasound2-dev`) plus X11/Wayland libs for winit.

## Layout

| File | Responsibility |
|------|----------------|
| `src/main.rs` | winit event loop; maps input to renderer/audio calls |
| `src/render.rs`, `src/cube.wgsl` | wgpu surface, pipeline, depth buffer, draws the cube; input: window, resize, time |
| `src/cube.rs`, `src/math.rs` | cube geometry and matrix helpers (pure, tested) |
| `src/audio.rs` | cpal output stream; `AudioOut::play_beep()`; explicit `AudioError` |
| `src/tone.rs` | pure sine-beep sample generator (tested); no audio asset needed |

## Behavior and limitations

- Audio init is attempted at startup; failure (no device, unsupported format) prints a message to stderr and the visuals keep running. Pressing Space retries initialization.
- Only `f32`, `i16`, `u16` device sample formats are supported; the beep is mono, copied to all channels.
- Single persistent output stream; replaying restarts the tone. No mixing, no streaming, no resampling (the tone is generated at the device rate).
- Fixed vsync, no gamepad/mouse input, no DPI/fullscreen handling beyond resize.
- Developed/smoke-tested on Windows; Linux/macOS are expected to work but untested.

## Integration seam for a future engine

`main.rs` is the only place that knows both winit and the subsystems. The seams are:
`Renderer::new(Arc<Window>)` / `resize` / `render(time)` (replace the cube draw with a frame/scene submission), and
`AudioOut::play_beep()` (replace with a sound-event/mixer API like qfusion's `S_StartSound`). The next step would be a small platform trait or channel
that the engine core drives, keeping winit as the event pump.
