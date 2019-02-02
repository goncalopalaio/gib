use super::gb::*;

mod ctx;
mod state;
mod utils;
mod views;

use ctx::UiContext;
use state::EmuState;
use views::{DebuggerView, DisassemblyView, MemEditView, MemMapView, View, WindowView};

use failure::Error;

use glium::{
    backend::Facade,
    texture::{ClientFormat, RawImage2d},
    Texture2d,
};
use imgui::{ImGuiCond, Ui};

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

const EMU_X_RES: usize = 160;
const EMU_Y_RES: usize = 144;

pub struct GuiState {
    debug: bool,
    should_quit: bool,
    file_dialog: Option<utils::FileDialog>,
    views: HashMap<View, Box<WindowView>>,
}

impl Default for GuiState {
    fn default() -> GuiState {
        GuiState {
            debug: false,
            should_quit: false,
            file_dialog: None,
            views: HashMap::new(),
        }
    }
}

pub struct EmuUi {
    ctx: Rc<RefCell<UiContext>>,
    gui: GuiState,

    emu: Option<EmuState>,
    vpu_buffer: Vec<u8>,
    vpu_texture: Option<imgui::ImTexture>,
}

impl EmuUi {
    pub fn new(debug: bool) -> EmuUi {
        let mut gui = GuiState::default();
        gui.debug = debug;

        EmuUi {
            ctx: Rc::from(RefCell::new(UiContext::new())),
            gui,

            emu: None,
            vpu_buffer: vec![0; EMU_X_RES * EMU_Y_RES * 4],
            vpu_texture: None,
        }
    }

    pub fn load_rom<P: AsRef<Path>>(&mut self, rom: P) -> Result<(), Error> {
        self.emu = Some(EmuState::new(rom)?);

        if self.gui.debug {
            let views = &mut self.gui.views;

            views.insert(View::Disassembly, box DisassemblyView::new());
            views.insert(View::Debugger, box DebuggerView::new());
            views.insert(View::MemEditor, box MemEditView::new());
            views.insert(View::MemMap, box MemMapView::new());
        }
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let mut last_frame = Instant::now();

        loop {
            let ctx = self.ctx.clone();
            let mut ctx = ctx.borrow_mut();

            ctx.poll_events();

            if self.gui.should_quit || ctx.should_quit() {
                return Ok(());
            }

            let now = Instant::now();
            let delta = now - last_frame;
            let delta_s = delta.as_secs() as f32 + delta.subsec_nanos() as f32 / 1_000_000_000.0;
            last_frame = now;

            if let Some(ref mut emu) = self.emu {
                emu.do_step(&mut self.vpu_buffer[..]);

                let new_screen = Texture2d::new(
                    ctx.display.get_context(),
                    RawImage2d {
                        data: Cow::Borrowed(&self.vpu_buffer[..]),
                        width: EMU_X_RES as u32,
                        height: EMU_Y_RES as u32,
                        format: ClientFormat::U8U8U8U8,
                    },
                )
                .unwrap();

                if let Some(texture) = self.vpu_texture {
                    ctx.renderer.textures().replace(texture, new_screen);
                } else {
                    self.vpu_texture = Some(ctx.renderer.textures().insert(new_screen));
                }
            }

            ctx.render(delta_s, |ui| self.draw(delta_s, ui));
        }
    }

    fn draw(&mut self, delta_s: f32, ui: &Ui) {
        self.draw_menu_bar(delta_s, ui);

        if self.emu.is_some() {
            self.draw_screen(ui);
        }

        if let Some(ref mut emu) = self.emu {
            self.gui.views.retain(|_, view| view.draw(ui, emu));
        }
    }

    fn draw_menu_bar(&mut self, delta_s: f32, ui: &Ui) {
        let emu_running = self.emu.is_some();

        self.draw_file_dialog(delta_s, ui);

        ui.main_menu_bar(|| {
            ui.menu(im_str!("Emulator")).build(|| {
                if ui.menu_item(im_str!("Load ROM...")).build() {
                    self.gui.file_dialog = Some(utils::FileDialog::new("Load ROM..."));
                }

                ui.separator();

                if ui.menu_item(im_str!("Reset")).enabled(emu_running).build() {
                    if let Some(ref mut emu) = self.emu {
                        emu.reset().expect("error during reset");
                    }
                }

                self.gui.should_quit = ui.menu_item(im_str!("Exit")).build();
            });

            ui.menu(im_str!("Hardware")).build(|| {
                if ui
                    .menu_item(im_str!("Memory Map"))
                    .enabled(emu_running)
                    .build()
                {
                    self.gui
                        .views
                        .entry(View::MemMap)
                        .or_insert_with(|| box MemMapView::new());
                }

                ui.menu_item(im_str!("VPU")).enabled(emu_running).build();
                ui.menu_item(im_str!("APU")).enabled(emu_running).build();
                ui.menu_item(im_str!("TIM")).enabled(emu_running).build();
                ui.menu_item(im_str!("ITR")).enabled(emu_running).build();
            });

            ui.menu(im_str!("Debugging")).build(|| {
                if ui
                    .menu_item(im_str!("Debugger"))
                    .enabled(emu_running)
                    .build()
                {
                    self.gui
                        .views
                        .entry(View::Debugger)
                        .or_insert_with(|| box DebuggerView::new());
                }

                if ui
                    .menu_item(im_str!("Disassembler"))
                    .enabled(emu_running)
                    .build()
                {
                    self.gui
                        .views
                        .entry(View::Disassembly)
                        .or_insert_with(|| box DisassemblyView::new());
                }

                if ui
                    .menu_item(im_str!("Memory Editor"))
                    .enabled(emu_running)
                    .build()
                {
                    self.gui
                        .views
                        .entry(View::MemEditor)
                        .or_insert_with(|| box MemEditView::new());
                }
            })
        });
    }

    fn draw_file_dialog(&mut self, delta_s: f32, ui: &Ui) {
        let mut fd_closed = false;
        let mut fd_chosen = None;

        if let Some(ref mut fd) = self.gui.file_dialog {
            fd.build(delta_s, ui, |res| {
                fd_closed = true;
                fd_chosen = res;
            });
        }
        if fd_closed {
            self.gui.file_dialog = None;
        }

        if let Some(ref rom_file) = fd_chosen {
            if let Err(evt) = self.load_rom(rom_file) {
                ui.popup_modal(im_str!("Error loading ROM")).build(|| {
                    ui.text(format!("{}", evt));
                });
                ui.open_popup(im_str!("Error loading ROM"));
            }
        }
    }

    fn draw_screen(&mut self, ui: &Ui) {
        ui.window(im_str!("Screen"))
            .size(
                (EMU_X_RES as f32 + 15.0, EMU_Y_RES as f32 + 40.0),
                ImGuiCond::FirstUseEver,
            )
            .position((720.0, 30.0), ImGuiCond::FirstUseEver)
            .resizable(false)
            .build(|| {
                if let Some(texture) = self.vpu_texture {
                    ui.image(texture, (EMU_X_RES as f32, EMU_Y_RES as f32))
                        .build();
                }
            });
    }
}
