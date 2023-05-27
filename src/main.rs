use std::iter;
use std::time::Instant;

use chrono::Timelike;
use egui_wgpu::renderer::ScreenDescriptor;
use egui_wgpu::Renderer;
use egui_winit::State;

use wgpu::InstanceDescriptor;
use winit::event::Event::*;
use winit::event_loop::ControlFlow;
use winit::window::Icon;

const INITIAL_WIDTH: u32 = 1920;
const INITIAL_HEIGHT: u32 = 1080;

use clap::Parser;

/// Simple program to greet a person
#[derive(Parser, Debug)]
struct Args {
    /// Show the egui demo app
    #[arg(long, default_value_t = true)]
    demo: bool,

    /// Enable puffin profiling
    #[arg(long, default_value_t = true)]
    profile: bool,

    /// Show puffin profiling
    #[arg(long)]
    profiler: bool,
}

/// A custom event type for the winit app.
enum Event {
    RequestRedraw,
}

/// This is the repaint signal type that egui needs for requesting a repaint from another thread.
/// It sends the custom RequestRedraw event to the winit event loop.
struct ExampleRepaintSignal(std::sync::Mutex<winit::event_loop::EventLoopProxy<Event>>);

impl epi::backend::RepaintSignal for ExampleRepaintSignal {
    fn request_repaint(&self) {
        self.0.lock().unwrap().send_event(Event::RequestRedraw).ok();
    }
}

fn load_icon() -> Option<Icon> {
    let file = include_bytes!("../assets/icon.ico");
    let icon_dir = ico::IconDir::read(std::io::Cursor::new(file)).unwrap();
    let image = icon_dir.entries()[0].decode().unwrap();
    Icon::from_rgba(image.rgba_data().to_vec(), image.width(), image.height()).ok()
}

/// A simple egui + wgpu + winit based example.
fn main() {
    let args = Args::parse();

    puffin::set_scopes_on(args.profile);

    let event_loop = winit::event_loop::EventLoopBuilder::<Event>::with_user_event().build();
    let window = winit::window::WindowBuilder::new()
        .with_decorations(true)
        .with_resizable(true)
        .with_transparent(false)
        .with_title("egui-wgpu-winit example")
        .with_inner_size(winit::dpi::PhysicalSize {
            width: INITIAL_WIDTH,
            height: INITIAL_HEIGHT,
        })
        .with_window_icon(load_icon())
        .build(&event_loop)
        .unwrap();

    let instance_descriptor = InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..InstanceDescriptor::default()
    };
    let instance = wgpu::Instance::new(instance_descriptor);
    let surface = unsafe { instance.create_surface(&window).unwrap() };

    // WGPU 0.11+ support force fallback (if HW implementation not supported), set it to true or false (optional).
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .unwrap();

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            features: wgpu::Features::default(),
            limits: wgpu::Limits::default(),
            label: None,
        },
        None,
    ))
    .unwrap();

    let capabilities = surface.get_capabilities(&adapter);
    let surface_format = *capabilities.formats.iter().find(|f| f.is_srgb()).unwrap();

    let size = window.inner_size();
    let mut surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: capabilities.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &surface_config);

    let mut state = State::new(&window);
    state.set_pixels_per_point(window.scale_factor() as f32);

    // We use the egui_wgpu_backend crate as the render backend.
    let mut egui_rpass = Renderer::new(&device, surface_format, None, 1);

    // Display the demo application that ships with egui.
    let mut demo_app = args.demo.then(egui_demo_lib::DemoWindows::default);

    let context = egui::Context::default();

    let _start_time = Instant::now();
    event_loop.run(move |event, _, control_flow| {
        // Pass the winit events to the platform integration.
        if let WindowEvent { event, .. } = &event {
            let response = state.on_event(&context, event);
            if response.repaint {
                window.request_redraw();
            }
            if response.consumed {
                return;
            }
        }

        match event {
            RedrawRequested(..) => {
                puffin::GlobalProfiler::lock().new_frame();
                let output_frame = match surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(wgpu::SurfaceError::Outdated) => {
                        // This error occurs when the app is minimized on Windows.
                        // Silently return here to prevent spamming the console with:
                        // "The underlying surface has changed, and therefore the swap chain must be updated"
                        return;
                    }
                    Err(e) => {
                        eprintln!("Dropped frame with error: {}", e);
                        return;
                    }
                };
                let output_view = output_frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                // Begin to draw the UI frame.
                let input = state.take_egui_input(&window);
                context.begin_frame(input);

                // Draw the demo application.
                if let Some(demo_app) = &mut demo_app {
                    puffin::profile_scope!("demo_app");
                    demo_app.ui(&context);
                }
                if args.profiler {
                    puffin_egui::profiler_window(&context);
                }

                // End the UI frame. We could now handle the output and draw the UI with the backend.
                let full_output = context.end_frame();
                let paint_jobs = context.tessellate(full_output.shapes);

                state.handle_platform_output(&window, &context, full_output.platform_output);

                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("encoder"),
                });

                // Upload all resources for the GPU.
                let screen_descriptor = ScreenDescriptor {
                    size_in_pixels: [surface_config.width, surface_config.height],
                    pixels_per_point: window.scale_factor() as f32,
                };
                let tdelta: egui::TexturesDelta = full_output.textures_delta;
                for (tid, deltas) in tdelta.set {
                    egui_rpass.update_texture(&device, &queue, tid, &deltas);
                }

                egui_rpass.update_buffers(
                    &device,
                    &queue,
                    &mut encoder,
                    &paint_jobs,
                    &screen_descriptor,
                );

                let color_attach = wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: Default::default(),
                };
                let renderpass_descriptor = wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(color_attach)],
                    ..Default::default()
                };
                let mut render_pass = encoder.begin_render_pass(&renderpass_descriptor);

                egui_rpass.render(&mut render_pass, &paint_jobs, &screen_descriptor);

                drop(render_pass);

                // Submit the commands.
                queue.submit(iter::once(encoder.finish()));

                // Redraw egui
                output_frame.present();

                for tid in tdelta.free {
                    egui_rpass.free_texture(&tid);
                }

                // Support reactive on windows only, but not on linux.
                // if _output.needs_repaint {
                //     *control_flow = ControlFlow::Poll;
                // } else {
                //     *control_flow = ControlFlow::Wait;
                // }
            }
            MainEventsCleared | UserEvent(Event::RequestRedraw) => {
                window.request_redraw();
            }
            WindowEvent { event, .. } => match event {
                winit::event::WindowEvent::Resized(size) => {
                    // Resize with 0 width and height is used by winit to signal a minimize event on Windows.
                    // See: https://github.com/rust-windowing/winit/issues/208
                    // This solves an issue where the app would panic when minimizing on Windows.
                    if size.width > 0 && size.height > 0 {
                        surface_config.width = size.width;
                        surface_config.height = size.height;
                        surface.configure(&device, &surface_config);
                    }
                }
                winit::event::WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            _ => (),
        }
    });
}

/// Time of day as seconds since midnight. Used for clock in demo app.
pub fn seconds_since_midnight() -> f64 {
    let time = chrono::Local::now().time();
    time.num_seconds_from_midnight() as f64 + 1e-9 * (time.nanosecond() as f64)
}
