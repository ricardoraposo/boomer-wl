mod capture;
mod config;
mod navigation;
mod render;

use std::{
    path::{Path, PathBuf},
    process,
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState, SimpleGlobal},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            BTN_LEFT, CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

use capture::Captures;
use config::Config;
use navigation::{Camera, Flashlight, Mouse, Vec2};
use render::{Image, Scene};

const MAX_FRAME_DT: f32 = 0.05;
const IDLE_FRAME_DT: f32 = 1.0 / 60.0;
const MIN_MOTION_DT: f32 = 1.0 / 240.0;
const FLICK_TIMEOUT_MS: u32 = 60;
const VELOCITY_SMOOTHING: f32 = 0.6;
const PIXELS_PER_SCROLL_STEP: f64 = 10.0;
const MAX_SWAP_BUFFERS: usize = 3;

const USAGE: &str = "Usage: boomer [OPTIONS]
  -d, --delay <seconds: float>  delay execution of the program by provided <seconds>
  -h, --help                    show this help and exit
      --new-config [filepath]   generate a new default config at [filepath]
  -c, --config <filepath>       use config at <filepath>
  -V, --version                 show the current version and exit";

struct Options {
    config_path: PathBuf,
    delay: f32,
}

fn default_config_path() -> PathBuf {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config_dir.join("boomer").join("config")
}

fn usage_quit() -> ! {
    eprintln!("{USAGE}");
    process::exit(1)
}

fn generate_config(path: PathBuf) -> ! {
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("could not create {}: {e}", dir.display());
        process::exit(1)
    }
    if path.exists() {
        eprint!("File {} already exists. Replace it? [yn] ", path.display());
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if answer.trim() != "y" {
            eprintln!("Disaster prevented");
            process::exit(1)
        }
    }
    match std::fs::write(&path, Config::default().to_string()) {
        Ok(()) => {
            eprintln!("Generated config at {}", path.display());
            process::exit(0)
        }
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            process::exit(1)
        }
    }
}

fn parse_args() -> Options {
    let mut options = Options {
        config_path: default_config_path(),
        delay: 0.0,
    };
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-d" | "--delay" => {
                let value = args.next().unwrap_or_else(|| {
                    eprintln!("No value is provided for {arg}");
                    usage_quit()
                });
                options.delay = value.parse().unwrap_or_else(|_| {
                    eprintln!("`{value}` is not a number of seconds");
                    usage_quit()
                });
            }
            "-c" | "--config" => {
                options.config_path = args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("No value is provided for {arg}");
                    usage_quit()
                });
            }
            "--new-config" => {
                let path = args
                    .next_if(|next| !next.starts_with('-'))
                    .map(PathBuf::from)
                    .unwrap_or_else(|| options.config_path.clone());
                generate_config(path)
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                process::exit(0)
            }
            "-V" | "--version" => {
                println!("boomer-wl {}", env!("CARGO_PKG_VERSION"));
                process::exit(0)
            }
            _ => {
                eprintln!("Unknown flag `{arg}`");
                usage_quit()
            }
        }
    }
    options
}

fn load_config(path: &Path) -> Config {
    if !path.exists() {
        eprintln!("{} doesn't exist. Using default values.", path.display());
        return Config::default();
    }
    Config::load(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1)
    })
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("boomer: {message}");
    process::exit(1)
}

fn main() {
    let options = parse_args();
    if options.delay > 0.0 {
        std::thread::sleep(Duration::from_secs_f32(options.delay));
    }
    let config = load_config(&options.config_path);
    eprintln!("Using config:\n{config}");

    let conn = Connection::connect_to_env().unwrap_or_else(|e| fail(format!("no Wayland display: {e}")));
    let (globals, mut queue) = registry_queue_init(&conn).unwrap_or_else(|e| fail(e));
    let qh = queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).unwrap_or_else(|_| fail("wl_compositor is missing"));
    let layer_shell = LayerShell::bind(&globals, &qh)
        .unwrap_or_else(|_| fail("the compositor does not support wlr-layer-shell"));
    let shm = Shm::bind(&globals, &qh).unwrap_or_else(|_| fail("wl_shm is missing"));
    let screencopy: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .unwrap_or_else(|_| fail("the compositor does not support wlr-screencopy"));
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh)
        .unwrap_or_else(|_| fail("the compositor does not support wp_viewporter"));
    let pool = SlotPool::new(4, &shm).unwrap_or_else(|e| fail(e));

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        viewporter,
        pool,
        buffers: Vec::new(),
        captures: None,
        screenshots: Vec::new(),
        active: None,
        layer: None,
        viewport: None,
        logical_size: (0, 0),
        keyboard: None,
        pointer: None,
        ctrl: false,
        config,
        config_path: options.config_path,
        camera: Camera::default(),
        mouse: Mouse::default(),
        flashlight: Flashlight::default(),
        mirror: false,
        last_motion_time: 0,
        scroll_remainder: 0.0,
        frame_pending: false,
        dirty: false,
        last_frame: None,
        exit: false,
    };

    queue.roundtrip(&mut app).unwrap_or_else(|e| fail(e));
    queue.roundtrip(&mut app).unwrap_or_else(|e| fail(e));

    let outputs: Vec<_> = app.output_state.outputs().collect();
    if outputs.is_empty() {
        fail("no outputs to capture");
    }
    app.captures = Some(Captures::start(&screencopy, outputs, &qh));
    while !app.captures.as_ref().is_some_and(Captures::is_finished) {
        queue.blocking_dispatch(&mut app).unwrap_or_else(|e| fail(e));
    }
    app.screenshots = app
        .captures
        .take()
        .unwrap()
        .into_images()
        .unwrap_or_else(|e| fail(e));
    screencopy.destroy();
    if app.screenshots.len() == 1 {
        app.active = Some(0);
    }

    let surface = app.compositor.create_surface(&qh);
    let viewport = app
        .viewporter
        .get()
        .unwrap_or_else(|_| fail("wp_viewporter is gone"))
        .get_viewport(&surface, &qh, ());
    let layer = layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("boomer"), None);
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer.set_size(0, 0);
    layer.commit();
    app.layer = Some(layer);
    app.viewport = Some(viewport);

    while !app.exit {
        queue.blocking_dispatch(&mut app).unwrap_or_else(|e| fail(e));
    }
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    viewporter: SimpleGlobal<WpViewporter, 1>,
    pool: SlotPool,
    buffers: Vec<Buffer>,

    captures: Option<Captures>,
    screenshots: Vec<(wl_output::WlOutput, Image)>,
    active: Option<usize>,

    layer: Option<LayerSurface>,
    viewport: Option<WpViewport>,
    logical_size: (u32, u32),
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    ctrl: bool,

    config: Config,
    config_path: PathBuf,
    camera: Camera,
    mouse: Mouse,
    flashlight: Flashlight,
    mirror: bool,
    last_motion_time: u32,
    scroll_remainder: f64,

    frame_pending: bool,
    dirty: bool,
    last_frame: Option<Instant>,
    exit: bool,
}

impl App {
    fn active_image(&self) -> Option<&Image> {
        self.active.map(|index| &self.screenshots[index].1)
    }

    fn window_size(&self) -> Vec2 {
        self.active_image().map_or(Vec2::ZERO, |image| {
            Vec2::new(image.width as f32, image.height as f32)
        })
    }

    fn to_buffer_coords(&self, (x, y): (f64, f64)) -> Vec2 {
        let size = self.window_size();
        let (logical_w, logical_h) = self.logical_size;
        if logical_w == 0 || logical_h == 0 || size == Vec2::ZERO {
            return Vec2::new(x as f32, y as f32);
        }
        Vec2::new(
            x as f32 * size.x / logical_w as f32,
            y as f32 * size.y / logical_h as f32,
        )
    }

    fn is_animating(&self) -> bool {
        self.camera.is_animating(&self.mouse) || self.flashlight.is_animating()
    }

    fn request_redraw(&mut self, qh: &QueueHandle<Self>) {
        self.dirty = true;
        if !self.frame_pending {
            self.draw(qh);
        }
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(layer) = self.layer.as_ref() else { return };
        let Some(viewport) = self.viewport.as_ref() else {
            return;
        };
        let (logical_w, logical_h) = self.logical_size;
        if logical_w == 0 || logical_h == 0 {
            return;
        }
        let surface = layer.wl_surface().clone();

        let Some(index) = self.active else {
            self.draw_placeholder(&surface);
            return;
        };

        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(IDLE_FRAME_DT, |t| (now - t).as_secs_f32().min(MAX_FRAME_DT));
        self.last_frame = Some(now);
        let window_size = self.window_size();
        self.camera.update(&self.config, dt, &self.mouse, window_size);
        self.flashlight.update(dt);

        let image = &self.screenshots[index].1;
        let (width, height) = (image.width, image.height);
        let stride = width as i32 * 4;
        let free = self
            .buffers
            .iter()
            .position(|b| b.canvas(&mut self.pool).is_some());
        let slot = match free {
            Some(slot) => slot,
            None if self.buffers.len() < MAX_SWAP_BUFFERS => {
                match self
                    .pool
                    .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
                {
                    Ok((buffer, _)) => {
                        self.buffers.push(buffer);
                        self.buffers.len() - 1
                    }
                    Err(e) => fail(format!("could not allocate a frame buffer: {e}")),
                }
            }
            None => {
                self.dirty = true;
                return;
            }
        };
        let buffer = &self.buffers[slot];
        let canvas = buffer
            .canvas(&mut self.pool)
            .expect("buffer was checked to be free");
        let (_, pixels, _) = unsafe { canvas.align_to_mut::<u32>() };
        let scene = Scene {
            image,
            camera: &self.camera,
            flashlight: &self.flashlight,
            cursor: self.mouse.curr,
            mirror: self.mirror,
        };
        render::draw(pixels, width, height, &scene);

        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(&surface).expect("frame buffer attach");
        viewport.set_destination(logical_w as i32, logical_h as i32);
        surface.commit();
        self.frame_pending = true;
        self.dirty = false;
    }

    fn draw_placeholder(&mut self, surface: &wl_surface::WlSurface) {
        let (logical_w, logical_h) = self.logical_size;
        let Ok((buffer, canvas)) = self.pool.create_buffer(1, 1, 4, wl_shm::Format::Argb8888) else {
            return;
        };
        canvas.fill(0);
        buffer.attach_to(surface).expect("placeholder attach");
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(logical_w as i32, logical_h as i32);
        }
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();
    }

    fn scroll_up(&mut self) {
        if self.ctrl && self.flashlight.is_enabled {
            self.flashlight.grow();
        } else {
            self.camera.delta_scale += self.config.scroll_speed;
            self.camera.scale_pivot = self.mouse.curr;
        }
    }

    fn scroll_down(&mut self) {
        if self.ctrl && self.flashlight.is_enabled {
            self.flashlight.shrink();
        } else {
            self.camera.delta_scale -= self.config.scroll_speed;
            self.camera.scale_pivot = self.mouse.curr;
        }
    }

    fn reset_view(&mut self) {
        self.camera = Camera::default();
        self.mirror = false;
    }

    fn toggle_mirror(&mut self) {
        let window_w = self.window_size().x;
        self.camera.position.x =
            -self.camera.position.x - 2.0 * (self.mouse.curr.x - window_w * 0.5) / self.camera.scale;
        self.mirror = !self.mirror;
    }

    fn reload_config(&mut self) {
        if !self.config_path.exists() {
            return;
        }
        match Config::load(&self.config_path) {
            Ok(config) => self.config = config,
            Err(e) => eprintln!("{e}"),
        }
    }

    fn scroll_steps(&mut self, scroll: smithay_client_toolkit::seat::pointer::AxisScroll) -> i32 {
        if scroll.value120 != 0 {
            self.scroll_remainder += scroll.value120 as f64 / 120.0;
        } else if scroll.discrete != 0 {
            self.scroll_remainder += scroll.discrete as f64;
        } else {
            self.scroll_remainder += scroll.absolute / PIXELS_PER_SCROLL_STEP;
        }
        let steps = self.scroll_remainder.trunc();
        self.scroll_remainder -= steps;
        steps as i32
    }

    fn handle_motion(&mut self, position: Vec2, time: u32) {
        self.mouse.curr = position;
        if self.mouse.drag {
            let delta = self.camera.world(self.mouse.prev) - self.camera.world(self.mouse.curr);
            self.camera.position += delta;
            let elapsed = (time.wrapping_sub(self.last_motion_time) as f32 / 1000.0).max(MIN_MOTION_DT);
            let instant_velocity = delta / elapsed;
            self.camera.velocity =
                self.camera.velocity * (1.0 - VELOCITY_SMOOTHING) + instant_velocity * VELOCITY_SMOOTHING;
        }
        self.last_motion_time = time;
        self.mouse.prev = self.mouse.curr;
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty || self.is_animating() {
            self.draw(qh);
        } else {
            self.last_frame = None;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        if self.active.is_some() {
            return;
        }
        self.active = self
            .screenshots
            .iter()
            .position(|(captured, _)| captured == output);
        if self.active.is_some() {
            self.request_redraw(qh);
        }
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.logical_size = configure.new_size;
        self.request_redraw(qh);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme::<_, ()>(
                    qh,
                    &seat,
                    self.shm.wl_shm(),
                    cursor_surface,
                    ThemeSpec::default(),
                )
                .ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.pointer().release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::equal | Keysym::plus | Keysym::KP_Add => self.scroll_up(),
            Keysym::minus | Keysym::KP_Subtract => self.scroll_down(),
            Keysym::_0 | Keysym::KP_0 => self.reset_view(),
            Keysym::q | Keysym::Escape => {
                self.exit = true;
                return;
            }
            Keysym::r => self.reload_config(),
            Keysym::m => self.toggle_mirror(),
            Keysym::f => self.flashlight.is_enabled = !self.flashlight.is_enabled,
            _ => return,
        }
        self.request_redraw(qh);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.ctrl = modifiers.ctrl;
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(layer) = self.layer.as_ref() else { return };
        let surface = layer.wl_surface().clone();
        for event in events.iter().filter(|event| event.surface == surface) {
            let position = self.to_buffer_coords(event.position);
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.pointer {
                        let _ = pointer.set_cursor(conn, CursorIcon::Default);
                    }
                    self.mouse.curr = position;
                    self.mouse.prev = position;
                }
                PointerEventKind::Leave { .. } => self.mouse.drag = false,
                PointerEventKind::Motion { time } => self.handle_motion(position, time),
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    self.mouse.prev = self.mouse.curr;
                    self.mouse.drag = true;
                    self.camera.velocity = Vec2::ZERO;
                }
                PointerEventKind::Release {
                    button: BTN_LEFT,
                    time,
                    ..
                } => {
                    self.mouse.drag = false;
                    if time.wrapping_sub(self.last_motion_time) > FLICK_TIMEOUT_MS {
                        self.camera.velocity = Vec2::ZERO;
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let steps = self.scroll_steps(vertical);
                    for _ in 0..steps.abs() {
                        if steps < 0 {
                            self.scroll_up()
                        } else {
                            self.scroll_down()
                        }
                    }
                }
                _ => {}
            }
        }
        self.request_redraw(qh);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for App {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        &mut self.viewporter
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

wayland_client::delegate_noop!(App: WpViewporter);
wayland_client::delegate_noop!(App: WpViewport);
delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
