use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

use wgpui::renderer::Renderer;
use wgpui::viz::badge::{BadgeTone, tone_color as badge_color};
use wgpui::viz::chart::{HistoryChartSeries, paint_history_chart_body};
use wgpui::viz::feed::{EventFeedRow, paint_event_feed_body};
use wgpui::viz::panel;
use wgpui::viz::provenance::{ProvenanceTone, tone_color as provenance_color};
use wgpui::viz::theme as viz_theme;
use wgpui::viz::topology::{TopologyNodeState, node_state_color};
use wgpui::{Bounds, Hsla, PaintContext, Point, Quad, Scene, Size, TextSystem, theme};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const WINDOW_WIDTH: f64 = 1180.0;
const WINDOW_HEIGHT: f64 = 760.0;

pub fn run_overlay_demo() -> Result<(), String> {
    ensure_overlay_support()?;
    let event_loop = EventLoop::new()
        .map_err(|error| format!("failed to create overlay event loop: {error}"))?;
    let mut app = OverlayDemoApp::default();
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("overlay event loop failed: {error}"))
}

pub fn ensure_overlay_support() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let has_display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if !has_display {
            return Err(String::from(
                "experimental overlay requires a desktop session with DISPLAY or WAYLAND_DISPLAY",
            ));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        return Err(String::from(
            "experimental overlay is only supported on macOS, Linux, or Windows desktop builds",
        ));
    }

    Ok(())
}

#[derive(Default)]
struct OverlayDemoApp {
    state: Option<OverlayRenderState>,
}

struct OverlayRenderState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    text_system: TextSystem,
    started_at: Instant,
}

impl ApplicationHandler for OverlayDemoApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window_attrs = Window::default_attributes()
            .with_title("Probe Experimental WGPUI Overlay")
            .with_inner_size(winit::dpi::LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
        let window = match event_loop.create_window(window_attrs) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create overlay window: {error}");
                event_loop.exit();
                return;
            }
        };

        let state = match pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                ..Default::default()
            });
            let surface = instance
                .create_surface(window.clone())
                .map_err(|error| format!("failed to create overlay surface: {error}"))?;
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .ok_or_else(|| String::from("failed to find a GPU adapter for the overlay"))?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|error| format!("failed to create overlay device: {error}"))?;

            let size = window.inner_size();
            let surface_caps = surface.get_capabilities(&adapter);
            let surface_format = surface_caps
                .formats
                .iter()
                .find(|format| format.is_srgb())
                .copied()
                .unwrap_or(surface_caps.formats[0]);
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: surface_caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&device, &config);

            let renderer = Renderer::new(&device, surface_format);
            Ok::<OverlayRenderState, String>(OverlayRenderState {
                window,
                surface,
                renderer,
                device,
                queue,
                config,
                text_system: TextSystem::new(1.0),
                started_at: Instant::now(),
            })
        }) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
                return;
            }
        };

        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape))
                {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                state.config.width = size.width.max(1);
                state.config.height = size.height.max(1);
                state.surface.configure(&state.device, &state.config);
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                render_overlay_frame(state, event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn render_overlay_frame(state: &mut OverlayRenderState, event_loop: &ActiveEventLoop) {
    let width = state.config.width as f32;
    let height = state.config.height as f32;
    let elapsed = state.started_at.elapsed().as_secs_f32();

    let mut scene = Scene::new();
    build_overlay_demo_scene(&mut scene, &mut state.text_system, width, height, elapsed);

    let output = match state.surface.get_current_texture() {
        Ok(output) => output,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            state.surface.configure(&state.device, &state.config);
            state.window.request_redraw();
            return;
        }
        Err(wgpu::SurfaceError::OutOfMemory) => {
            event_loop.exit();
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => {
            state.window.request_redraw();
            return;
        }
        Err(wgpu::SurfaceError::Other) => {
            state.window.request_redraw();
            return;
        }
    };
    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("probe_overlay_demo_encoder"),
        });

    state
        .renderer
        .resize(&state.queue, Size::new(width, height), 1.0);
    if state.text_system.is_dirty() {
        state.renderer.update_atlas(
            &state.queue,
            state.text_system.atlas_data(),
            state.text_system.atlas_size(),
        );
        state.text_system.mark_clean();
    }
    state.renderer.prepare(
        &state.device,
        &state.queue,
        &scene,
        state.window.scale_factor() as f32,
    );
    state.renderer.render(&mut encoder, &view);
    state.queue.submit(std::iter::once(encoder.finish()));
    output.present();
}

fn build_overlay_demo_scene(
    scene: &mut Scene,
    text_system: &mut TextSystem,
    width: f32,
    height: f32,
    time: f32,
) {
    let mut paint = PaintContext::new(scene, text_system, 1.0);
    let root = Bounds::new(0.0, 0.0, width, height);
    paint
        .scene
        .draw_quad(Quad::new(root).with_background(theme::bg::APP));

    let phase = (time * 0.16).fract();
    let title = paint.text.layout_mono(
        "PROBE EXPERIMENTAL WGPUI OVERLAY",
        Point::new(26.0, 18.0),
        12.0,
        viz_theme::series::LOSS.with_alpha(0.96),
    );
    paint.scene.draw_text(title);
    let subtitle = paint.text.layout(
        "Ctrl+G launches this sidecar from `probe tui`. Esc or close the window to dismiss it.",
        Point::new(26.0, 38.0),
        11.0,
        theme::text::SECONDARY,
    );
    paint.scene.draw_text(subtitle);

    let left = Bounds::new(24.0, 72.0, width * 0.34 - 28.0, 220.0);
    let chart = Bounds::new(
        left.max_x() + 18.0,
        72.0,
        width - left.max_x() - 42.0,
        360.0,
    );
    let bottom = Bounds::new(
        24.0,
        chart.max_y() + 18.0,
        width - 48.0,
        height - chart.max_y() - 42.0,
    );

    panel::paint_shell(left, viz_theme::track::PGOLF, &mut paint);
    panel::paint_title(left, "OVERLAY STATUS", viz_theme::track::PGOLF, &mut paint);
    panel::paint_texture(left, viz_theme::track::PGOLF, phase, &mut paint);
    paint_overlay_badges(left, &mut paint);

    panel::paint_shell(chart, viz_theme::series::LOSS, &mut paint);
    panel::paint_title(
        chart,
        "TOKEN CADENCE DEMO",
        viz_theme::series::LOSS,
        &mut paint,
    );
    let token_cadence = build_series(56.0, time, 7.0, 18.0, 0.9);
    let tool_load = build_series(3.2, time * 0.8, 0.42, 1.1, 1.4);
    let attention = build_series(0.52, time * 1.2, 0.14, 0.28, 0.35);
    let chart_series = [
        HistoryChartSeries {
            label: "tokens/sec",
            values: token_cadence.as_slice(),
            color: viz_theme::series::LOSS,
            fill_alpha: 0.18,
            line_alpha: 0.78,
        },
        HistoryChartSeries {
            label: "tool load",
            values: tool_load.as_slice(),
            color: viz_theme::series::PROVENANCE,
            fill_alpha: 0.10,
            line_alpha: 0.88,
        },
        HistoryChartSeries {
            label: "attention",
            values: attention.as_slice(),
            color: viz_theme::series::HARDWARE,
            fill_alpha: 0.0,
            line_alpha: 0.92,
        },
    ];
    paint_history_chart_body(
        chart,
        viz_theme::series::LOSS,
        phase,
        Some("probe.overlay.demo // synthetic cadence and tool-load telemetry"),
        Some("This is an experimental non-portable visual sidecar, not the default TUI renderer."),
        "No overlay history available.",
        &chart_series,
        &mut paint,
    );

    panel::paint_shell(bottom, viz_theme::series::EVENTS, &mut paint);
    panel::paint_title(bottom, "EVENT FEED", viz_theme::series::EVENTS, &mut paint);
    paint_event_feed_body(
        bottom,
        viz_theme::series::EVENTS,
        phase,
        "No overlay events recorded.",
        &overlay_feed_rows(time),
        &mut paint,
    );
}

fn overlay_feed_rows(time: f32) -> [EventFeedRow<'static>; 4] {
    let pulse = ((time * 1.4).sin() + 1.0) * 0.5;
    [
        EventFeedRow {
            label: Cow::Borrowed("overlay_hotkey"),
            detail: Cow::Borrowed(
                "The Probe TUI launched a WGPUI sidecar without surrendering terminal ownership.",
            ),
            color: badge_color(BadgeTone::Live),
        },
        EventFeedRow {
            label: Cow::Borrowed("renderer_mode"),
            detail: Cow::Borrowed(
                "This lane is intentionally experimental and should remain capability-gated.",
            ),
            color: provenance_color(ProvenanceTone::Evidence),
        },
        EventFeedRow {
            label: Cow::Borrowed("focus_model"),
            detail: Cow::Borrowed(
                "Terminal input stays in Probe while the sidecar owns pointer and window focus.",
            ),
            color: node_state_color(TopologyNodeState::Warning),
        },
        EventFeedRow {
            label: Cow::Borrowed("pulse"),
            detail: Cow::Owned(format!(
                "Synthetic telemetry pulse {:.2} keeps the history chart visibly alive for review.",
                pulse
            )),
            color: badge_color(BadgeTone::TrackXtrain),
        },
    ]
}

fn build_series(base: f32, time: f32, amplitude: f32, drift: f32, frequency: f32) -> Vec<f32> {
    (0..36)
        .map(|index| {
            let sample = time - (35 - index) as f32 * 0.18;
            let wave = (sample * frequency).sin() * amplitude;
            let modulation = (sample * (frequency * 0.41 + 0.37)).cos() * amplitude * 0.28;
            (base + wave + modulation + sample * drift * 0.02).max(0.01)
        })
        .collect()
}

fn paint_overlay_badges(bounds: Bounds, paint: &mut PaintContext) {
    let lines = [
        "Mode: experimental sidecar",
        "Host: Probe TUI + separate WGPUI window",
        "Dismiss: Esc or window close",
        "Purpose: richer visual telemetry proof, not terminal replacement",
    ];
    let mut y = bounds.origin.y + 38.0;
    for line in lines {
        paint.scene.draw_text(paint.text.layout(
            line,
            Point::new(bounds.origin.x + 16.0, y),
            11.0,
            theme::text::PRIMARY,
        ));
        y += 18.0;
    }

    let badges = [
        ("EXPERIMENTAL", badge_color(BadgeTone::Warning)),
        ("TUI", badge_color(BadgeTone::TrackPgolf)),
        ("WGPUI", badge_color(BadgeTone::TrackHomegolf)),
        ("SIDECAR", badge_color(BadgeTone::TrackXtrain)),
    ];
    let mut x = bounds.origin.x + 16.0;
    let badge_y = bounds.origin.y + 138.0;
    for (label, color) in badges {
        draw_badge(Bounds::new(x, badge_y, 104.0, 26.0), label, color, paint);
        x += 112.0;
        if x + 104.0 > bounds.max_x() - 12.0 {
            x = bounds.origin.x + 16.0;
        }
    }
}

fn draw_badge(bounds: Bounds, label: &str, color: Hsla, paint: &mut PaintContext) {
    paint.scene.draw_quad(
        Quad::new(bounds)
            .with_background(color.with_alpha(0.12))
            .with_border(color.with_alpha(0.42), 1.0)
            .with_corner_radius(6.0),
    );
    paint.scene.draw_text(paint.text.layout_mono(
        label,
        Point::new(bounds.origin.x + 10.0, bounds.origin.y + 7.0),
        10.0,
        color.with_alpha(0.94),
    ));
}
