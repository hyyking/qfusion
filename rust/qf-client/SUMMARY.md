# qfusion client crate - Summary

The desktop binary owns the Winit event loop and keyboard input, WGPU BSP rendering, and CPAL audio output. It loads a BSP from a direct file path or a PK3 entry, initializes the engine's noclip player at a map spawn, and advances simulation at a fixed tick.

## What was tested

Automated workspace checks (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, `cargo build --workspace`, `cargo test --workspace`):

- `math`: view and perspective transforms.
- `tone`: sample count, bounded amplitude, fades, zero duration.
- Engine integration: map parsing, collision, player spawn and fixed-step movement tests.

The installed `wbomb1` map was parsed through the engine smoke test and BSP inspector.

## Runtime limitations

- The renderer remains a BSP preview with no textures or lightmaps; patches are tessellated.
- Audio is a generated test beep, not a game mixer.
- Window resizing is supported; mouse/gamepad input, fullscreen, and DPI behavior are not implemented.
- Tests do not require a display, GPU, or audio device.
