//! The interactive emulator window.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant as StdInstant};

use quireink_platform::{
    ButtonState, InputEvent, Point, PointerEvent, PointerPhase, PointerTool, buttons,
};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::budget;
use crate::device::Device;
use crate::headless;

const BACKGROUND: u32 = 0x0030_3030;
/// Frame interval while an animation runs.
const ANIMATION_FRAME: Duration = Duration::from_millis(30);

/// Keyboard keys emulating device buttons.
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::PageDown, buttons::PAGE_FORWARD),
    (KeyCode::Space, buttons::PAGE_FORWARD),
    (KeyCode::PageUp, buttons::PAGE_BACK),
    (KeyCode::ArrowUp, buttons::UP),
    (KeyCode::ArrowDown, buttons::DOWN),
    (KeyCode::ArrowLeft, buttons::LEFT),
    (KeyCode::ArrowRight, buttons::RIGHT),
    (KeyCode::Enter, buttons::CONFIRM),
    (KeyCode::Escape, buttons::BACK),
    (KeyCode::Backspace, buttons::BACK),
    (KeyCode::KeyM, buttons::MENU),
    (KeyCode::KeyH, buttons::HOME),
    (KeyCode::KeyP, buttons::POWER),
];

pub fn run(device: Device, screenshot_dir: PathBuf) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        device,
        screenshot_dir,
        window: None,
        surface: None,
        cursor: PhysicalPosition::new(0.0, 0.0),
        pointer_down: false,
        error: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    match app.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct App {
    device: Device,
    screenshot_dir: PathBuf,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    cursor: PhysicalPosition<f64>,
    pointer_down: bool,
    error: Option<String>,
}

/// Where the panel is drawn inside the window.
#[derive(Debug, Clone, Copy)]
struct Viewport {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn update_title(&self) {
        if let Some(window) = &self.window {
            let screen = self
                .device
                .kernel
                .active_screen()
                .map(ToString::to_string)
                .unwrap_or_default();
            window.set_title(&format!(
                "QuireInk Emulator — {} — {} — {}",
                self.device.profile.device.name,
                screen,
                budget::stats()
            ));
        }
    }

    fn viewport(&self, size: PhysicalSize<u32>) -> Viewport {
        let (pw, ph) = (
            u32::from(self.device.panel.width()),
            u32::from(self.device.panel.height()),
        );
        let scale = (size.width as f64 / pw as f64).min(size.height as f64 / ph as f64);
        let width = ((pw as f64 * scale) as u32).max(1);
        let height = ((ph as f64 * scale) as u32).max(1);
        Viewport {
            x: size.width.saturating_sub(width) / 2,
            y: size.height.saturating_sub(height) / 2,
            width,
            height,
        }
    }

    /// Converts a window position to logical panel coordinates.
    fn to_panel(&self, position: PhysicalPosition<f64>) -> Option<Point> {
        let window = self.window.as_ref()?;
        let v = self.viewport(window.inner_size());
        let x = position.x - f64::from(v.x);
        let y = position.y - f64::from(v.y);
        if x < 0.0 || y < 0.0 || x >= f64::from(v.width) || y >= f64::from(v.height) {
            return None;
        }
        let px = x * f64::from(self.device.panel.width()) / f64::from(v.width);
        let py = y * f64::from(self.device.panel.height()) / f64::from(v.height);
        Some(Point::new(px as i32, py as i32))
    }

    fn pointer(&mut self, phase: PointerPhase, point: Point) {
        self.device.input(InputEvent::Pointer(PointerEvent {
            id: 0,
            phase,
            point,
            tool: PointerTool::Finger,
            pressure: 1000,
        }));
        self.after_input();
    }

    fn after_input(&mut self) {
        self.update_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn redraw(&mut self) -> Result<(), String> {
        let Some(window) = self.window.clone() else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        let v = self.viewport(size);
        let pixels = self.device.panel.compose();
        let (pw, ph) = (
            usize::from(self.device.panel.width()),
            usize::from(self.device.panel.height()),
        );
        let Some(surface) = &mut self.surface else {
            return Ok(());
        };
        surface.resize(w, h).map_err(|e| e.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        buffer.fill(BACKGROUND);

        let columns: Vec<usize> = (0..v.width as usize)
            .map(|x| (x * pw / v.width as usize).min(pw - 1))
            .collect();
        let stride = size.width as usize;
        for y in 0..v.height as usize {
            let source_row = (y * ph / v.height as usize).min(ph - 1) * pw;
            let target_row = (y + v.y as usize) * stride + v.x as usize;
            for (x, &column) in columns.iter().enumerate() {
                buffer[target_row + x] = pixels[source_row + column];
            }
        }
        buffer.present().map_err(|e| e.to_string())
    }

    fn screenshot(&self) {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = self
            .screenshot_dir
            .join(format!("{}-{millis}.png", self.device.profile.device.id));
        match headless::screenshot(&self.device, &path) {
            Ok(()) => println!("screenshot saved to {}", path.display()),
            Err(error) => eprintln!("screenshot failed: {error}"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (pw, ph) = (
            f64::from(self.device.panel.width()),
            f64::from(self.device.panel.height()),
        );
        // Start at a comfortable size; the window is freely resizable.
        let scale = (900.0 / ph).min(1.0);
        let attributes = Window::default_attributes()
            .with_title("QuireInk Emulator")
            .with_inner_size(winit::dpi::LogicalSize::new(pw * scale, ph * scale));
        let result = event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())
            .and_then(|window| {
                let window = Rc::new(window);
                let context =
                    softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
                let surface = softbuffer::Surface::new(&context, window.clone())
                    .map_err(|e| e.to_string())?;
                Ok((window, surface))
            });
        match result {
            Ok((window, surface)) => {
                self.window = Some(window);
                self.surface = Some(surface);
                self.device.start();
                self.after_input();
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => self.after_input(),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.redraw() {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                match code {
                    KeyCode::F2 if pressed => {
                        self.device.panel.show_regions = !self.device.panel.show_regions;
                    }
                    KeyCode::F3 if pressed => {
                        self.device.panel.animate = !self.device.panel.animate;
                    }
                    KeyCode::F12 if pressed => self.screenshot(),
                    _ => {
                        if event.repeat {
                            return;
                        }
                        let Some(&(_, name)) = KEYS
                            .iter()
                            .find(|(key, name)| *key == code && self.device.has_button(name))
                        else {
                            return;
                        };
                        let state = if pressed {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        };
                        self.device.input(InputEvent::Button { name, state });
                        self.after_input();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if self.pointer_down
                    && let Some(point) = self.to_panel(position)
                {
                    self.pointer(PointerPhase::Move, point);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if !self.device.profile.device.input.touch {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        if let Some(point) = self.to_panel(self.cursor) {
                            self.pointer_down = true;
                            self.pointer(PointerPhase::Down, point);
                        }
                    }
                    ElementState::Released if self.pointer_down => {
                        self.pointer_down = false;
                        match self.to_panel(self.cursor) {
                            Some(point) => self.pointer(PointerPhase::Up, point),
                            None => self.pointer(PointerPhase::Cancel, Point::new(0, 0)),
                        }
                    }
                    ElementState::Released => {}
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let before = self.device.wake_at;
        self.device.tick_if_due();
        if self.device.wake_at != before {
            self.after_input();
        }

        let mut deadline: Option<StdInstant> = None;
        if self.device.panel.is_animating() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            deadline = Some(StdInstant::now() + ANIMATION_FRAME);
        }
        if let Some(wake) = self.device.wake_at {
            let wait = Duration::from_millis(wake.since(self.device.now()));
            let at = StdInstant::now() + wait;
            deadline = Some(deadline.map_or(at, |d| d.min(at)));
        }
        event_loop.set_control_flow(match deadline {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}
