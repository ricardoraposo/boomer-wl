# boomer-wl

A Rust and Wayland port of [tsoding/boomer](https://github.com/tsoding/boomer), the zoomer application for Linux.

It captures the output under focus with `wlr-screencopy`, shows it on a fullscreen `wlr-layer-shell` overlay and lets you zoom, pan and shine a flashlight on it. Rendering is done on the CPU (rayon) into shared-memory buffers, so no GPU stack is needed.

Requires a wlroots-based compositor (sway, Hyprland, river, labwc, Wayfire, niri, ...) that exposes `zwlr_screencopy_manager_v1`, `zwlr_layer_shell_v1` and `wp_viewporter`. GNOME and KDE do not expose screencopy.

## Build

```sh
cargo build --release
install -Dm755 target/release/boomer ~/.local/bin/boomer
```

Sway binding example:

```
bindsym $mod+z exec boomer
```

## Controls

| Control                                   | Description                                                   |
|-------------------------------------------|---------------------------------------------------------------|
| <kbd>0</kbd>                              | Reset the application state (position, scale, velocity, etc). |
| <kbd>q</kbd> or <kbd>ESC</kbd>            | Quit the application.                                         |
| <kbd>r</kbd>                              | Reload configuration.                                         |
| <kbd>m</kbd>                              | Mirror the image around the cursor.                           |
| <kbd>f</kbd>                              | Toggle flashlight effect.                                     |
| <kbd>=</kbd> / <kbd>-</kbd>               | Zoom in / out.                                                |
| Drag with left mouse button               | Move the image around.                                        |
| Scroll wheel                              | Zoom in/out.                                                  |
| <kbd>Ctrl</kbd> + Scroll wheel            | Change the radius of the flashlight.                          |

## Configuration

Same format and location as the original: `$XDG_CONFIG_HOME/boomer/config` (default `~/.config/boomer/config`).

```
min_scale = 0.01
scroll_speed = 1.5
drag_friction = 6.0
scale_friction = 4.0
```

Generate one with `boomer --new-config [filepath]`.

## Differences from the original

- Multi-monitor: every output is captured up front, and the overlay shows the capture of the output the compositor opens it on (the focused one).
- HiDPI and fractional scaling: the buffer is rendered at native resolution and scaled with `wp_viewporter`.
- No `--windowed`, `select` or `live` modes. Rotated outputs are not handled yet.
