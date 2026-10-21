//! The Wayland frontend.
//!
//! No toolkits: we talk to the compositor through smithay-client-toolkit, draw
//! every pixel ourselves with tiny-skia into a shared-memory buffer, and shape
//! text with cosmic-text. By default the window is a **layer-shell overlay**
//! (`zwlr_layer_shell_v1`): it floats above everything and grabs the keyboard,
//! exactly like fuzzel, on niri, Hyprland, Sway, mango and every other
//! layer-shell compositor. The
//! `position` config key places it in the middle of the screen (default),
//! pinned to the top or pinned to the bottom. When layer shell is unavailable
//! (or `layer false` / `--no-layer`), foto falls back to a plain tileable
//! `xdg_toplevel` window.

mod render;

use crate::config::Position;
use crate::state::{run_action, Action, App, Flow, Key};
use crate::thumbs::ThumbEngine;
use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::calloop::{channel, EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    activation::{ActivationHandler, ActivationState, RequestData},
    compositor::{CompositorHandler, CompositorState},
    delegate_activation, delegate_compositor, delegate_keyboard, delegate_output,
    delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use std::sync::Arc;
use std::time::Duration;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1,
    wp_fractional_scale_v1::WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

pub fn run(conn: Connection, app: App, force_plain_window: bool) -> Result<()> {
    let (globals, event_queue) = registry_queue_init(&conn).context("wayland registry")?;
    let qh = event_queue.handle();

    let mut event_loop = EventLoop::try_new().context("calloop event loop")?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .context("wayland event source")?;

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor missing")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm missing")?;

    // Background thumbnail loading: worker threads decode images off the
    // render thread and ping this channel whenever new thumbnails are ready,
    // so `--files` opens instantly instead of decoding wallpapers inside the
    // draw loop.
    let (thumb_ping, thumb_source) = channel::channel::<()>();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 3))
        .unwrap_or(2);
    let mut app = app;
    app.engine = Some(Arc::new(ThumbEngine::new(thumb_ping, workers)));
    loop_handle
        .insert_source(thumb_source, |_, _, gui: &mut Gui| {
            gui.dirty = true; // thumbnails published — repaint
        })
        .map_err(|e| anyhow::anyhow!("thumbnail channel source: {e}"))?;

    // --files: feed every discovered path into the thumbnail queue in one
    // pass so the grid streams in while the workers decode, instead of
    // waiting for each cell to scroll into view. lookup() dedupes, and
    // results arrive through the calloop channel above.
    if app.files_mode {
        if let Some(engine) = app.engine.clone() {
            let size = if app.cfg.layout == crate::config::Layout::Grid {
                app.cfg.grid_size
            } else {
                app.cfg.icon_size
            };
            for entry in &app.entries {
                if let crate::state::EntryKind::Image(f) = &entry.kind {
                    engine.lookup(&f.path, size);
                }
            }
        }
    }

    // Layer-shell overlay (the fuzzel behavior), with xdg fallback.
    let layer_shell = if app.cfg.layer && !force_plain_window {
        // the vendored protocol XML knows layer-shell up to version 5
        globals
            .bind::<ZwlrLayerShellV1, Gui, ()>(&qh, 1..=5, ())
            .ok()
    } else {
        None
    };

    let surface = compositor.create_surface(&qh);
    let shell = if let Some(layer_shell) = layer_shell {
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None, // compositor picks the focused output, like fuzzel
            Layer::Overlay,
            "foto".into(),
            &qh,
            (),
        );
        // Placement: with no anchor at all the compositor centers the surface
        // on the output — the layer-shell way of saying "middle of the screen"
        // (this is how wofi/walker center themselves). Top/bottom pin it to
        // that edge, shifted inward by `margin-y`.
        let (anchor, top, bottom) = match app.cfg.position {
            Position::Center => (Anchor::empty(), 0, 0),
            Position::Top => (Anchor::Top, app.cfg.margin_y, 0),
            Position::Bottom => (Anchor::Bottom, 0, app.cfg.margin_y),
        };
        layer_surface.set_anchor(anchor);
        layer_surface.set_margin(top, 0, bottom, 0); // top right bottom left
        layer_surface.set_exclusive_zone(-1); // overlap panels instead of pushing them
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer_surface.set_size(app.cfg.width, app.cfg.height);
        surface.commit(); // maps the layer surface; a configure follows
        Shell::Layer {
            surface,
            layer_surface,
        }
    } else {
        let xdg_shell = XdgShell::bind(&globals, &qh).context("xdg_shell missing")?;
        let window = xdg_shell.create_window(surface, WindowDecorations::RequestClient, &qh);
        window.set_title("foto");
        window.set_app_id("foto");
        window.commit(); // maps the window; a configure event follows
        Shell::Xdg { window }
    };

    // HiDPI: bind wp_viewporter + wp_fractional_scale_manager_v1 when the
    // compositor advertises them. With both present the surface renders at
    // the compositor's preferred (possibly fractional) scale: buffers are
    // allocated at ceil(logical * scale) physical pixels and the viewport
    // scales them back down to the logical size, so text stays sharp on
    // HiDPI outputs. When either global is missing we fall back cleanly to
    // scale 1 (the pre-HiDPI behavior).
    let viewporter = globals.bind::<WpViewporter, Gui, ()>(&qh, 1..=1, ()).ok();
    let fractional_manager = globals
        .bind::<WpFractionalScaleManagerV1, Gui, ()>(&qh, 1..=1, ())
        .ok();
    let (viewport, fractional_scale) = match (&viewporter, &fractional_manager) {
        (Some(vp_manager), Some(fs_manager)) => {
            let surface = shell.wl_surface();
            let viewport = vp_manager.get_viewport(surface, &qh, ());
            let fractional = fs_manager.get_fractional_scale(surface, &qh, ());
            (Some(viewport), Some(fractional))
        }
        _ => (None, None), // scale-1 fallback path
    };

    // xdg-activation asks the compositor nicely to focus us (KDE requires it).
    // A layer-shell surface with Exclusive keyboard interactivity does not
    // need it — the compositor hands us focus directly.
    let activation = if matches!(shell, Shell::Xdg { .. }) {
        ActivationState::bind(&globals, &qh).ok()
    } else {
        None
    };

    if let (Some(activation), Shell::Xdg { window }) = (activation.as_ref(), &shell) {
        activation.request_token(
            &qh,
            RequestData {
                seat_and_serial: None,
                surface: Some(window.wl_surface().clone()),
                app_id: Some(String::from("foto")),
            },
        );
    }
    let pool = SlotPool::new((app.cfg.width * app.cfg.height * 4) as usize, &shm)
        .context("wl_shm pool")?;

    let mut gui = Gui {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        activation,
        shell,
        pool,
        buffer: None,
        width: app.cfg.width,
        height: app.cfg.height,
        scale: 1.0,
        viewporter,
        viewport,
        fractional_scale,
        app,
        renderer: render::Renderer::new(),
        modifiers: Modifiers::default(),
        dirty: false,
        should_exit: false,
        first_configure: true,
        _loop_handle: loop_handle, // keeps the repeat timer alive
    };

    loop {
        event_loop
            .dispatch(Duration::from_millis(200), &mut gui)
            .context("event loop dispatch")?;

        if gui.should_exit {
            break;
        }
        if gui.dirty {
            gui.dirty = false;
            gui.draw();
        }
    }
    Ok(())
}

/// Which shell protocol our surface lives under.
enum Shell {
    Xdg {
        window: Window,
    },
    Layer {
        surface: wl_surface::WlSurface,
        // kept alive so the surface keeps its layer role and keeps receiving
        // configure events; we never need to touch it again after setup
        #[allow(dead_code)]
        layer_surface: ZwlrLayerSurfaceV1,
    },
}

impl Shell {
    fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            Shell::Xdg { window } => window.wl_surface(),
            Shell::Layer { surface, .. } => surface,
        }
    }

    fn commit(&self) {
        self.wl_surface().commit();
    }
}

struct Gui {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    activation: Option<ActivationState>,
    shell: Shell,
    pool: SlotPool,
    buffer: Option<Buffer>,
    /// logical (surface-local) size — what the compositor scales to the output
    width: u32,
    height: u32,
    /// preferred scale from wp_fractional_scale_v1 (1.0 when unsupported).
    /// Buffers are allocated at ceil(logical * scale) physical pixels.
    scale: f64,
    /// HiDPI protocol objects; `viewport` is None on the scale-1 fallback
    /// path (either global missing). The globals are kept alive so the
    /// per-surface objects stay valid — same keep-alive pattern as
    /// `Shell::Layer::layer_surface`.
    #[allow(dead_code)] // kept for proxy lifetime, never read back
    viewporter: Option<WpViewporter>,
    viewport: Option<WpViewport>,
    #[allow(dead_code)] // kept for proxy lifetime, never read back
    fractional_scale: Option<WpFractionalScaleV1>,
    app: App,
    renderer: render::Renderer,
    modifiers: Modifiers,
    dirty: bool,
    should_exit: bool,
    first_configure: bool,
    _loop_handle: LoopHandle<'static, Self>,
}

/// Physical buffer dimension for a logical dimension at `scale`:
/// rounded up, never zero (a zero-sized wl_shm buffer is a protocol error).
fn buffer_dim(logical: u32, scale: f64) -> u32 {
    ((logical as f64 * scale).ceil() as u32).max(1)
}

impl Gui {
    /// Render one frame and present it to the compositor.
    fn draw(&mut self) {
        let (logical_w, logical_h) = (self.width, self.height);
        // physical pixels: the pixmap and wl_shm buffer are scaled up, the
        // viewport squashes them back to the logical size before commit
        let (buf_w, buf_h) = (
            buffer_dim(logical_w, self.scale),
            buffer_dim(logical_h, self.scale),
        );
        let stride = buf_w as i32 * 4;

        let mut pixmap = render::pixmap_for(buf_w, buf_h);

        // Icons/thumbnails are loaded lazily here, so `app` must be mutable.
        self.renderer
            .render(&mut self.app, &mut pixmap, self.scale as f32);

        let buffer = self.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(buf_w as i32, buf_h as i32, stride, wl_shm::Format::Argb8888)
                .expect("create buffer")
                .0
        });

        // The compositor may still hold the previous buffer; use a second one.
        let canvas = match self.pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let (second, canvas) = self
                    .pool
                    .create_buffer(buf_w as i32, buf_h as i32, stride, wl_shm::Format::Argb8888)
                    .expect("create buffer");
                *buffer = second;
                canvas
            }
        };

        // tiny-skia rows are RGBA; Wayland ARGB8888 expects BGRA in memory.
        for (chunk, px) in canvas
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(pixmap.pixels())
        {
            chunk[0] = px.blue();
            chunk[1] = px.green();
            chunk[2] = px.red();
            chunk[3] = px.alpha();
        }

        let surface = self.shell.wl_surface();
        surface.damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        if let Some(viewport) = &self.viewport {
            // logical destination rectangle — the compositor maps the
            // physical buffer onto this before displaying (wp_viewporter)
            viewport.set_destination(logical_w as i32, logical_h as i32);
        }
        buffer.attach_to(surface).expect("buffer attach");
        self.shell.commit();
    }

    /// Translate a key event into our neutral Key and run it through the App.
    fn handle_key(&mut self, event: KeyEvent) {
        let key = keysym_to_key(&event, &self.modifiers);
        match self.app.on_key(key) {
            Flow::Continue => self.dirty = true,
            Flow::Exit => self.should_exit = true,
            Flow::Launch(action) => {
                match action {
                    Action::Shell(cmd) => {
                        let _ = run_action(&Action::Shell(cmd));
                    }
                    // dmenu: the chosen line goes to foto's own stdout, where
                    // the caller's pipe picks it up:
                    //   cliphist list | foto --dmenu | cliphist decode | wl-copy
                    Action::Print(line) => {
                        println!("{line}");
                    }
                }
                self.should_exit = true; // launcher closes after launching
            }
        }
    }
}

fn keysym_to_key(event: &KeyEvent, modifiers: &Modifiers) -> Key {
    match event.keysym {
        Keysym::Return | Keysym::KP_Enter => return Key::Enter,
        Keysym::Escape => return Key::Escape,
        Keysym::BackSpace => return Key::Backspace,
        Keysym::Up => return Key::Up,
        Keysym::Down => return Key::Down,
        Keysym::Left => return Key::Left,
        Keysym::Right => return Key::Right,
        Keysym::Tab => return Key::Tab,
        Keysym::ISO_Left_Tab => return Key::Tab,
        Keysym::Home => return Key::Top,
        Keysym::End => return Key::Bottom,
        _ => {}
    }

    if modifiers.ctrl {
        return match event.keysym.key_char() {
            Some('j') => Key::Down,
            Some('k') => Key::Up,
            Some('a') => Key::Top,    // readline: caret to start of line
            Some('e') => Key::Bottom, // readline: caret to end of line
            Some('w') => Key::WordDelete,
            Some('u') => Key::ClearLine,
            Some('c') => Key::Escape,
            _ => Key::Other,
        };
    }

    // Prefer the compositor's UTF-8 decoding (handles dead keys & layouts).
    if let Some(text) = &event.utf8 {
        if let Some(c) = text.chars().next() {
            return Key::Char(c);
        }
    }
    Key::Other
}

// ---------- smithay-client-toolkit plumbing ----------

impl CompositorHandler for Gui {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // v1 renders at scale 1; HiDPI crispening is on the roadmap.
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
        // moved to another output: the compositor sends a fresh
        // preferred_scale if the scale differs; repaint either way
        self.dirty = true;
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
        self.dirty = true;
    }
}

impl OutputHandler for Gui {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for Gui {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.should_exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        // Compositor-chosen size; 0 means "not specified, pick your own".
        let w = configure
            .new_size
            .0
            .map(|v| v.get())
            .unwrap_or(self.app.cfg.width);
        let h = configure
            .new_size
            .1
            .map(|v| v.get())
            .unwrap_or(self.app.cfg.height);
        if (w, h) != (self.width, self.height) {
            self.width = w;
            self.height = h;
            self.buffer = None; // force a new shm buffer at the new size
        }
        self.dirty = true;
        self.first_configure = false;
    }
}

impl ActivationHandler for Gui {
    type RequestData = RequestData;

    fn new_token(&mut self, token: String, _data: &Self::RequestData) {
        if let Some(activation) = self.activation.as_ref() {
            activation.activate::<Self>(self.shell.wl_surface(), token);
        }
    }
}

impl SeatHandler for Gui {
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
        // Grab the keyboard so typing reaches us.
        if capability == Capability::Keyboard {
            let _ = self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self._loop_handle.clone(),
                Box::new(|state: &mut Self, _, event| state.handle_key(event)),
            );
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Gui {
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
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
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
        _: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl ShmHandler for Gui {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(Gui);
delegate_output!(Gui);
delegate_shm!(Gui);
delegate_seat!(Gui);
delegate_keyboard!(Gui);
delegate_xdg_shell!(Gui);
delegate_xdg_window!(Gui);
delegate_activation!(Gui);
delegate_registry!(Gui);

// ---------- HiDPI: wp_viewporter + wp_fractional_scale_v1 ----------

impl Dispatch<WpFractionalScaleV1, ()> for Gui {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_fractional_scale_v1::Event;
        if let Event::PreferredScale { scale } = event {
            // the wire value is the numerator over a denominator of 120
            let new = scale as f64 / 120.0;
            if (new - state.scale).abs() > f64::EPSILON {
                state.scale = new;
                state.buffer = None; // re-allocate at the new physical size
                state.dirty = true;
            }
        }
    }
}

// both manager globals emit no events; bound only to create per-surface objects
impl Dispatch<WpViewporter, ()> for Gui {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: <WpViewporter as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Gui {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// wp_viewport emits no events either; the impl only satisfies the queue
impl Dispatch<WpViewport, ()> for Gui {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: <WpViewport as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl ProvidesRegistryState for Gui {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

// ---------- zwlr layer shell ----------

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Gui {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_layer_surface_v1::Event;
        match event {
            Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                // 0 means "keep what you asked for"
                let w = if width > 0 {
                    width
                } else {
                    state.app.cfg.width
                };
                let h = if height > 0 {
                    height
                } else {
                    state.app.cfg.height
                };
                if (w, h) != (state.width, state.height) {
                    state.width = w;
                    state.height = h;
                    state.buffer = None; // force a new shm buffer at the new size
                }
                state.dirty = true;
                state.first_configure = false;
            }
            Event::Closed => state.should_exit = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for Gui {
    fn event(
        _: &mut Self,
        _: &ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // the shell global emits no events; binding it only produces surfaces
    }
}

#[cfg(test)]
mod tests {
    use super::buffer_dim;

    #[test]
    fn buffer_dim_at_scale_one_is_identity() {
        assert_eq!(buffer_dim(640, 1.0), 640);
        assert_eq!(buffer_dim(440, 1.0), 440);
    }

    #[test]
    fn buffer_dim_integer_scales_multiply() {
        assert_eq!(buffer_dim(640, 2.0), 1280);
        assert_eq!(buffer_dim(440, 2.0), 880);
        assert_eq!(buffer_dim(640, 3.0), 1920);
    }

    #[test]
    fn buffer_dim_fractional_scales_round_up() {
        // 1.25x of 640 = 800 exactly; 1.25x of 449 = 561.25 -> 562
        assert_eq!(buffer_dim(640, 1.25), 800);
        assert_eq!(buffer_dim(449, 1.25), 562);
        // 1.5x of 641 = 961.5 -> 962 (never truncate — one row/column would
        // be missing at the edge otherwise)
        assert_eq!(buffer_dim(641, 1.5), 962);
        // 1.1x of 55 = 60.5 -> 61
        assert_eq!(buffer_dim(55, 1.1), 61);
    }

    #[test]
    fn buffer_dim_is_never_zero() {
        assert_eq!(buffer_dim(0, 1.0), 1);
        assert_eq!(buffer_dim(0, 2.0), 1);
        assert_eq!(buffer_dim(1, 0.5), 1);
    }
}
