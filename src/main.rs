//! Activity Journal Desktop: a tray icon, a window and a settings form over the Activity Journal
//! engine, which it runs unchanged. `egui` is driven directly on `winit` + `wgpu`, without `eframe`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_ui;
mod desktop_config;
mod engine_run;
mod exit_code;
mod journal_files;
mod lifecycle;
mod settings;
mod tray;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use egui::ViewportId;
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::app_ui::App;
use crate::engine_run::RunKind;
use crate::tray::{TrayMenu, TrayState};

/// Events from outside the window: engine output arrived, the tray, its menu.
#[derive(Debug)]
enum Wakeup {
    Repaint,
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

struct Surface {
    window: Arc<Window>,
    state: egui_winit::State,
    painter: egui_wgpu::winit::Painter,
}

struct Shell {
    context: egui::Context,
    app: App,
    start_hidden: bool,
    surface: Option<Surface>,
    tray: Option<(TrayIcon, TrayMenu)>,
    tray_state: Option<TrayState>,
}

impl Shell {
    fn show_window(&self) {
        if let Some(surface) = &self.surface {
            surface.window.set_visible(true);
            surface.window.set_minimized(false);
            surface.window.focus_window();
            surface.window.request_redraw();
        }
    }

    fn sync_tray(&mut self) {
        let state = self.app.tray_state();
        if self.tray_state != Some(state) {
            if let Some((tray, _)) = &self.tray {
                tray::show(tray, state);
            }
            self.tray_state = Some(state);
        }
    }

    fn paint(&mut self) {
        let Some(surface) = &mut self.surface else { return };
        let input = surface.state.take_egui_input(&surface.window);
        let app = &mut self.app;
        let mut output = self.context.run_ui(input, |ui| app.ui(ui));
        surface
            .state
            .handle_platform_output(&surface.window, std::mem::take(&mut output.platform_output));
        let primitives = self
            .context
            .tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        surface.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            [0.0, 0.0, 0.0, 1.0],
            &primitives,
            &mut output.textures_delta,
            Vec::new(),
            &surface.window,
        );
        let wants_repaint = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        if wants_repaint || self.app.is_running() {
            surface.window.request_redraw();
        }
    }
}

impl ApplicationHandler<Wakeup> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Activity Journal")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 640.0))
            .with_visible(!self.start_hidden)
            .with_window_icon(winit::window::Icon::from_rgba(tray::disc_rgba(TrayState::Idle, 32), 32, 32).ok());
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("the OS refused to create a window"),
        );
        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            self.context.clone(),
            egui_wgpu::WgpuConfiguration::default(),
            false,
            egui_wgpu::RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .expect("no graphics adapter could draw this window");
        let state = egui_winit::State::new(
            self.context.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );
        self.surface = Some(Surface { window, state, painter });

        match tray::build(self.app.tray_state()) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("tray icon unavailable: {e}"),
        }
        if self.tray.is_none() {
            self.show_window();
        }
        self.sync_tray();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wakeup) {
        match event {
            Wakeup::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })
            | Wakeup::Tray(TrayIconEvent::DoubleClick { .. }) => self.show_window(),
            Wakeup::Repaint | Wakeup::Tray(_) => {}
            Wakeup::Menu(menu) => {
                let Some((_, ids)) = &self.tray else { return };
                let id = menu.id();
                if *id == ids.open {
                    self.show_window();
                } else if *id == ids.checkpoint {
                    self.app.start_run(RunKind::Checkpoint);
                } else if *id == ids.final_run {
                    self.app.start_run(RunKind::Final);
                } else if *id == ids.quit {
                    event_loop.exit();
                }
            }
        }
        if let Some(surface) = &self.surface {
            surface.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(surface) = &mut self.surface else { return };
        let response = surface.state.on_window_event(&surface.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if self.tray.is_some() {
                    surface.window.set_visible(false);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    surface.painter.on_window_resized(ViewportId::ROOT, width, height);
                }
                surface.window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.paint(),
            _ => {
                if response.repaint {
                    surface.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let next = self.app.tick();
        self.sync_tray();
        event_loop.set_control_flow(match next {
            Some(at) if at > Instant::now() => ControlFlow::WaitUntil(at),
            _ => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(surface) = &mut self.surface {
            surface.painter.destroy();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let event_loop = EventLoop::<Wakeup>::with_user_event()
        .build()
        .expect("cannot start the event loop");

    let tray_proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(Wakeup::Tray(event));
    }));
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(Wakeup::Menu(event));
    }));
    let repaint_proxy = event_loop.create_proxy();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = repaint_proxy.send_event(Wakeup::Repaint);
    });

    let mut shell = Shell {
        context: egui::Context::default(),
        app: App::new(desktop_config::engine_root(&args), wake),
        start_hidden: args.iter().any(|a| a == "--hidden"),
        surface: None,
        tray: None,
        tray_state: None,
    };
    event_loop.run_app(&mut shell).expect("the event loop failed");
}
