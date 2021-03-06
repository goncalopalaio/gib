use imgui::{FontGlyphRange, ImFontConfig, ImGui, ImVec4, Ui};
use imgui_gfx_renderer::{Renderer, Shaders};

use gfx_core::handle::{DepthStencilView, RenderTargetView};
use gfx_device_gl::{Device, Factory, Resources};
use glutin::{EventsLoop, GlWindow, VirtualKeyCode as Key};

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

type ColorFormat = gfx::format::Rgba8;
type DepthFormat = gfx::format::DepthStencil;

#[derive(Copy, Clone, PartialEq, Debug, Default)]
struct MouseState {
    pos: (i32, i32),
    pressed: (bool, bool, bool),
    wheel: f32,
}

pub struct UiContext {
    pub imgui: ImGui,

    pub window: GlWindow,
    pub device: Device,
    pub factory: Factory,
    pub renderer: Renderer<Resources>,
    pub main_color: RenderTargetView<Resources, ColorFormat>,
    pub main_depth: DepthStencilView<Resources, DepthFormat>,

    pub events_loop: Rc<RefCell<EventsLoop>>,
    pub hidpi_factor: f64,

    key_state: HashSet<Key>,
    should_quit: bool,
    focused: bool,
}

impl UiContext {
    /// Creates a new UI context with a window size of (width, height).
    pub fn new(width: f64, height: f64) -> UiContext {
        use glutin::{dpi::LogicalSize, ContextBuilder, WindowBuilder};

        let events_loop = EventsLoop::new();

        let context = ContextBuilder::new().with_vsync(true);
        let builder = WindowBuilder::new()
            .with_title("gib")
            .with_dimensions(LogicalSize::new(width, height));

        let (window, device, mut factory, main_color, main_depth) =
            gfx_window_glutin::init::<ColorFormat, DepthFormat>(builder, context, &events_loop)
                .expect("Failed to initalize graphics");

        let shaders = {
            let version = device.get_info().shading_language;
            if version.is_embedded {
                if version.major >= 3 {
                    Shaders::GlSlEs300
                } else {
                    Shaders::GlSlEs100
                }
            } else if version.major >= 4 {
                Shaders::GlSl400
            } else if version.major >= 3 {
                if version.minor >= 2 {
                    Shaders::GlSl150
                } else {
                    Shaders::GlSl130
                }
            } else {
                Shaders::GlSl110
            }
        };

        let mut imgui = ImGui::init();
        {
            // Fix incorrect colors with sRGB framebuffer
            fn imgui_gamma_to_linear(col: ImVec4) -> ImVec4 {
                let x = col.x.powf(2.2);
                let y = col.y.powf(2.2);
                let z = col.z.powf(2.2);
                let w = 1.0 - (1.0 - col.w).powf(2.2);
                ImVec4::new(x, y, z, w)
            }

            let style = imgui.style_mut();
            for col in 0..style.colors.len() {
                style.colors[col] = imgui_gamma_to_linear(style.colors[col]);
            }
        }
        imgui.set_ini_filename(None);

        let hidpi_factor = window.get_hidpi_factor().round();
        UiContext::load_fonts(&mut imgui, hidpi_factor);

        imgui_winit_support::configure_keys(&mut imgui);

        let renderer = Renderer::init(&mut imgui, &mut factory, shaders, main_color.clone())
            .expect("Failed to initialize renderer");

        UiContext {
            imgui,

            window,
            device,
            factory,
            renderer,
            main_color,
            main_depth,

            events_loop: Rc::new(RefCell::from(events_loop)),
            hidpi_factor,

            key_state: HashSet::new(),
            should_quit: false,
            focused: true,
        }
    }

    pub fn poll_events(&mut self) {
        let events_loop = self.events_loop.clone();

        events_loop.borrow_mut().poll_events(|event| {
            use glutin::{
                ElementState::Pressed,
                Event,
                WindowEvent::{CloseRequested, Focused, KeyboardInput, Resized},
            };

            imgui_winit_support::handle_event(
                &mut self.imgui,
                &event,
                self.window.get_hidpi_factor(),
                self.hidpi_factor,
            );

            if let Event::WindowEvent { event, .. } = event {
                match event {
                    Focused(focus) => self.focused = focus,
                    Resized(size) => {
                        gfx_window_glutin::update_views(
                            &self.window,
                            &mut self.main_color,
                            &mut self.main_depth,
                        );
                        self.renderer.update_render_target(self.main_color.clone());

                        // **This is required on macOS!**
                        self.window.resize(glutin::dpi::PhysicalSize::from_logical(
                            size,
                            self.hidpi_factor,
                        ));
                    }
                    CloseRequested => {
                        self.should_quit = true;
                    }
                    KeyboardInput { input, .. } => {
                        let pressed = input.state == Pressed;

                        if let Some(vk) = input.virtual_keycode {
                            if pressed {
                                self.key_state.insert(vk);
                            } else {
                                self.key_state.remove(&vk);
                            }
                        }
                    }
                    _ => (),
                }
            }
        });

        imgui_winit_support::update_mouse_cursor(&self.imgui, &self.window);
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn render<F>(&mut self, delta_s: f32, mut f: F)
    where
        F: FnMut(&Ui),
    {
        use gfx::Device;

        let frame_size =
            imgui_winit_support::get_frame_size(&self.window, self.hidpi_factor).unwrap();

        let ui = self.imgui.frame(frame_size, delta_s);

        f(&ui);

        let mut encoder: gfx::Encoder<_, _> = self.factory.create_command_buffer().into();

        encoder.clear(&self.main_color, [0.4, 0.5, 0.6, 1.0]);
        {
            self.renderer
                .render(ui, &mut self.factory, &mut encoder)
                .expect("Rendering failed");
        }
        encoder.flush(&mut self.device);

        self.window.swap_buffers().unwrap();
        self.device.cleanup();

        if !self.focused {
            // Throttle to 60 fps when in background, since macOS doesn't honor
            // V-Sync settings for non-visible windows, making the CPU shoot to 100%.
            std::thread::sleep(std::time::Duration::from_nanos(1_000_000_000 / 60));
        }
    }

    /// Returns the pressed state for the given virtual key.
    pub fn is_key_pressed(&self, key: Key) -> bool {
        self.key_state.contains(&key)
    }

    fn load_fonts(imgui: &mut ImGui, hidpi_factor: f64) {
        let font_size = (13.0 * hidpi_factor) as f32;

        imgui.fonts().add_default_font_with_config(
            ImFontConfig::new()
                .oversample_h(1)
                .pixel_snap_h(true)
                .size_pixels(font_size),
        );

        imgui.fonts().add_font_with_config(
            include_bytes!("../../res/mplus-1p-regular.ttf"),
            ImFontConfig::new()
                .merge_mode(true)
                .oversample_h(1)
                .pixel_snap_h(true)
                .size_pixels(font_size)
                .rasterizer_multiply(1.75),
            &FontGlyphRange::japanese(),
        );

        imgui.set_font_global_scale((1.0 / hidpi_factor) as f32);
    }
}
