# Spike 0005: nested 2× Wayland scaling

- **Status:** Successful integer-scale evidence
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Artifact:** `tools/run-wayland-scale-demo.py`
- **Evidence:** `docs/spikes/artifacts/0005/`

## Question

Does the candidate client respond to a real `wl_output` scale of 2 by rebuilding
font/raster state, allocating physical-size SHM buffers, setting the surface
buffer scale, damaging in buffer coordinates, and surviving resize?

## Method

The workspace-safe launcher builds the native example, refuses an occupied
workspace, then starts a nested Hyprland 0.55.4 compositor through the outer
Hyprland `hl.exec_cmd` workspace rule:

```bash
python tools/run-wayland-scale-demo.py \
  --workspace 8 --scale 2 \
  --capture /tmp/splinterm-row-scale-2.ppm
```

The nested compositor ran only on workspace 8 on DP-2. Its `WAYLAND-1` output
advertised scale 2. The example delayed capture until scale 2 was active.

## Results

Initial presentation:

```text
logical=910×486
buffer=1820×972
stride=7280
scale=2
```

After the nested compositor applied its tiled layout:

```text
logical=868×444
buffer=1736×888
stride=6944
scale=2
```

Both presentations satisfy:

```text
buffer width  = logical width  × 2
buffer height = logical height × 2
stride        = buffer width × 4
```

The scale transition rebuilt the deterministic row at 44 pixels with a 27×59
cell, 45-pixel baseline, and 26.4-pixel primary advance. The captured PPM was
1820×972 and had SHA-256
`ebca944bc2f19cc6452d8dfb7a7652a79d95cb379f6a8a464a7954c2b4b37940`.
A reviewed row crop and machine-readable dimensions are stored under
`artifacts/0005/`.

## Interpretation

- `wl_surface.set_buffer_scale(2)` is accepted by the nested compositor.
- Font metrics and glyph images are rerasterized rather than bitmap-upscaled.
- SHM allocation, stride, capture, and `damage_buffer` use physical buffer
  coordinates.
- A configure resize after the initial scaled frame produces a second correctly
  doubled buffer and does not terminate the client.
- The outer workspace remained isolated and was empty after cleanup.

## Fractional-scale decision

Fractional scaling will use `wp_fractional_scale_v1` together with
`wp_viewporter` through a small project-owned Wayland wrapper:

1. bind both globals when available and create one fractional-scale object and
   viewport per surface;
2. treat `preferred_scale` as fixed-point units of 1/120;
3. rerasterize fonts and renderer caches at that physical scale;
4. allocate `ceil(logical × preferred_scale / 120)` physical buffer dimensions;
5. use surface buffer scale 1 while fractional scaling is active;
6. set the viewport destination to the logical surface dimensions so the full
   physical buffer maps to the configured logical size; and
7. continue using buffer-coordinate damage.

If either protocol is unavailable, the client falls back to integer
`wl_output` scaling. Fractional support is not yet implemented and is not part
of the current production claim.
