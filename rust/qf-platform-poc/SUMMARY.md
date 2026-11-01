# POC summary

Goal: prove the desktop platform foundation for a Rust qfusion port: window + input (winit), rendering (wgpu), audio (cpal).

## What was tested

Automated (`cargo fmt --check`, `cargo clippy --all-targets`, `cargo build`, `cargo test`; all passed on Windows, Rust 1.97.1):

- `math`: identity, rotation, multiplication order, perspective depth range (0..1).
- `cube`: vertex/index counts, indices in bounds, outward counter-clockwise winding.
- `tone`: sample count, bounded non-silent amplitude, fades at both ends, zero duration.

Smoke run (Windows, one machine): the executable started, opened a window with the expected title, printed no errors to stderr, and exited with code 0 when the window was closed.

## Not verified (needs a human with a display and speakers)

- That the cube is visibly rendered, correctly colored/lit-free, and rotating smoothly.
- That the beep is audible after pressing Space, and the behavior when no output device exists (error path is implemented but not exercised).
- Window resize, minimize, and surface-loss recovery at runtime.
- Linux and macOS builds/runs, and other GPU backends (Vulkan/Metal/GL).
- Esc key and Space key handling were not driven by real key input in the smoke run.

## Left out on purpose

- Bevy or any full engine framework; gameplay, entities, game logic.
- BSP/map/model/texture loading, materials, lighting, camera controls.
- Networking, console, filesystem/VFS, config/cvars.
- Mouse/gamepad input, fullscreen, DPI handling, multiple windows.
- Audio mixing, streaming, resampling, 3D spatialization, file decoding (the tone is generated, not an asset).
- Integration with the existing C/C++ code or build; the crate is standalone.
- Tests that need a GPU or audio device (no CI wiring either).

## Integration seam

See README.md: `Renderer::new/resize/render` and `AudioOut::play_beep`, driven from `main.rs`.
