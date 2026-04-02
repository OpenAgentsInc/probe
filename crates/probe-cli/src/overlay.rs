use std::borrow::Cow;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::ValueEnum;
use crossterm::cursor::MoveTo;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use wgpui::renderer::Renderer;
use wgpui::viz::badge::{BadgeTone, tone_color as badge_color};
use wgpui::viz::chart::{HistoryChartSeries, paint_history_chart_body};
use wgpui::viz::feed::{EventFeedRow, paint_event_feed_body};
use wgpui::viz::panel;
use wgpui::viz::provenance::{ProvenanceTone, tone_color as provenance_color};
use wgpui::viz::theme as viz_theme;
use wgpui::viz::topology::{TopologyNodeState, node_state_color};
use wgpui::{
    Bounds, CaptureRequest, CaptureTarget, Hsla, PaintContext, Point, Quad, Scene, Size,
    TextSystem, capture_scene, theme,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const OVERLAY_WIDTH: u32 = 1180;
const OVERLAY_HEIGHT: u32 = 760;
const WINDOW_WIDTH: f64 = OVERLAY_WIDTH as f64;
const WINDOW_HEIGHT: f64 = OVERLAY_HEIGHT as f64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OverlayTarget {
    Auto,
    Terminal,
    Sidecar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayPresentation {
    SidecarWindow,
    TerminalInline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolvedOverlayTarget {
    Sidecar,
    Terminal(TerminalImageProtocol),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalImageProtocol {
    ITerm2InlineImage,
}

impl TerminalImageProtocol {
    fn label(self) -> &'static str {
        match self {
            Self::ITerm2InlineImage => "iTerm2 OSC 1337 inline image",
        }
    }
}

pub fn run_overlay_demo(target: OverlayTarget, from_tui_handoff: bool) -> Result<(), String> {
    if from_tui_handoff {
        return run_overlay_demo_with_tui_handoff(target);
    }
    run_overlay_demo_inner(target, false)
}

fn run_overlay_demo_with_tui_handoff(target: OverlayTarget) -> Result<(), String> {
    suspend_tui_terminal()?;
    let run_result = run_overlay_demo_inner(target, true);
    let restore_result = restore_tui_terminal();
    run_result.and(restore_result)
}

fn run_overlay_demo_inner(
    target: OverlayTarget,
    show_sidecar_fallback_notice: bool,
) -> Result<(), String> {
    match resolve_overlay_target(target)? {
        ResolvedOverlayTarget::Sidecar => {
            if show_sidecar_fallback_notice {
                print_sidecar_fallback_notice()?;
            }
            run_sidecar_overlay_demo()
        }
        ResolvedOverlayTarget::Terminal(protocol) => run_terminal_overlay_demo(protocol),
    }
}

fn resolve_overlay_target(target: OverlayTarget) -> Result<ResolvedOverlayTarget, String> {
    let terminal_protocol = detect_terminal_image_protocol();
    match target {
        OverlayTarget::Auto => {
            if let Some(protocol) = terminal_protocol {
                Ok(ResolvedOverlayTarget::Terminal(protocol))
            } else {
                ensure_sidecar_support()?;
                Ok(ResolvedOverlayTarget::Sidecar)
            }
        }
        OverlayTarget::Terminal => terminal_protocol
            .map(ResolvedOverlayTarget::Terminal)
            .ok_or_else(unsupported_terminal_overlay_message),
        OverlayTarget::Sidecar => {
            ensure_sidecar_support()?;
            Ok(ResolvedOverlayTarget::Sidecar)
        }
    }
}

fn unsupported_terminal_overlay_message() -> String {
    String::from(
        "experimental in-terminal overlay currently requires an interactive iTerm2 session with stdin/stdout attached directly to the terminal",
    )
}

fn detect_terminal_image_protocol() -> Option<TerminalImageProtocol> {
    detect_terminal_image_protocol_with_inputs(
        env::var("TERM_PROGRAM").ok().as_deref(),
        env::var("LC_TERMINAL").ok().as_deref(),
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        env::var_os("TMUX").is_some(),
        env::var_os("ZELLIJ").is_some(),
    )
}

fn detect_terminal_image_protocol_with_inputs(
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    in_tmux: bool,
    in_zellij: bool,
) -> Option<TerminalImageProtocol> {
    if !stdin_is_terminal || !stdout_is_terminal || in_tmux || in_zellij {
        return None;
    }

    if term_program == Some("iTerm.app") || lc_terminal == Some("iTerm2") {
        return Some(TerminalImageProtocol::ITerm2InlineImage);
    }

    None
}

fn run_terminal_overlay_demo(protocol: TerminalImageProtocol) -> Result<(), String> {
    let png_bytes = render_overlay_demo_png(OverlayPresentation::TerminalInline)?;
    let mut stdout = io::stdout();
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))
        .map_err(|error| format!("failed to prepare terminal overlay surface: {error}"))?;
    writeln!(stdout, "Probe experimental WGPUI terminal overlay")
        .map_err(|error| format!("failed to write terminal overlay header: {error}"))?;
    writeln!(stdout, "Protocol: {}.", protocol.label())
        .map_err(|error| format!("failed to write terminal overlay protocol line: {error}"))?;
    writeln!(
        stdout,
        "This is a capability-gated inline image proof, not the default TUI renderer."
    )
    .map_err(|error| format!("failed to write terminal overlay description: {error}"))?;
    writeln!(stdout)
        .map_err(|error| format!("failed to write terminal overlay spacing: {error}"))?;
    stdout
        .write_all(terminal_image_escape(protocol, png_bytes.as_slice()).as_bytes())
        .map_err(|error| format!("failed to write terminal image payload: {error}"))?;
    writeln!(stdout)
        .map_err(|error| format!("failed to write terminal overlay spacing: {error}"))?;
    writeln!(stdout)
        .map_err(|error| format!("failed to write terminal overlay spacing: {error}"))?;
    writeln!(stdout, "Press Enter to return.")
        .map_err(|error| format!("failed to write terminal overlay dismissal copy: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush terminal overlay output: {error}"))?;

    let mut dismissal = String::new();
    io::stdin()
        .read_line(&mut dismissal)
        .map_err(|error| format!("failed to read terminal overlay dismissal: {error}"))?;
    Ok(())
}

fn terminal_image_escape(protocol: TerminalImageProtocol, png_bytes: &[u8]) -> String {
    match protocol {
        TerminalImageProtocol::ITerm2InlineImage => iterm2_inline_image_escape(png_bytes),
    }
}

fn iterm2_inline_image_escape(png_bytes: &[u8]) -> String {
    let payload = STANDARD.encode(png_bytes);
    format!("\u{1b}]1337;File=inline=1;width=100%;preserveAspectRatio=1:{payload}\u{7}")
}

fn render_overlay_demo_png(presentation: OverlayPresentation) -> Result<Vec<u8>, String> {
    let capture_nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let output_path = env::temp_dir().join(format!(
        "probe_overlay_demo_{}_{}.png",
        std::process::id(),
        capture_nonce
    ));
    let manifest_path = output_path.with_extension("json");

    let mut scene = Scene::new();
    let mut text_system = TextSystem::new(1.0);
    build_overlay_demo_scene(
        &mut scene,
        &mut text_system,
        OVERLAY_WIDTH as f32,
        OVERLAY_HEIGHT as f32,
        0.28,
        presentation,
    );

    let mut request = CaptureRequest::new(
        CaptureTarget::AdHoc {
            name: String::from("probe_overlay_demo"),
        },
        OVERLAY_WIDTH,
        OVERLAY_HEIGHT,
        output_path.clone(),
    );
    request.manifest_path = Some(manifest_path.clone());

    capture_scene(&request, &scene, Some(&text_system)).map_err(|error| {
        format!("failed to render the experimental WGPUI overlay offscreen: {error}")
    })?;
    let png_bytes = fs::read(&output_path).map_err(|error| {
        format!(
            "failed to read rendered overlay PNG `{}`: {error}",
            output_path.display()
        )
    })?;
    let _ = fs::remove_file(&output_path);
    let _ = fs::remove_file(&manifest_path);
    Ok(png_bytes)
}

fn suspend_tui_terminal() -> Result<(), String> {
    let mut stdout = io::stdout();
    disable_raw_mode()
        .map_err(|error| format!("failed to disable raw mode for overlay handoff: {error}"))?;
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)
        .map_err(|error| format!("failed to leave the Probe TUI alternate screen: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush overlay handoff output: {error}"))?;
    Ok(())
}

fn restore_tui_terminal() -> Result<(), String> {
    let mut stdout = io::stdout();
    enable_raw_mode()
        .map_err(|error| format!("failed to re-enable raw mode after overlay handoff: {error}"))?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|error| format!("failed to restore the Probe TUI alternate screen: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush overlay restore output: {error}"))?;
    Ok(())
}

fn print_sidecar_fallback_notice() -> Result<(), String> {
    let mut stdout = io::stdout();
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))
        .map_err(|error| format!("failed to prepare sidecar fallback notice: {error}"))?;
    writeln!(stdout, "Probe experimental WGPUI overlay")
        .map_err(|error| format!("failed to write sidecar fallback header: {error}"))?;
    writeln!(
        stdout,
        "No supported in-terminal graphics protocol was detected, so Probe is falling back to the sidecar window."
    )
    .map_err(|error| format!("failed to write sidecar fallback detail: {error}"))?;
    writeln!(stdout, "Close the sidecar window to return to the TUI.")
        .map_err(|error| format!("failed to write sidecar fallback dismissal copy: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush sidecar fallback notice: {error}"))?;
    Ok(())
}

fn run_sidecar_overlay_demo() -> Result<(), String> {
    ensure_sidecar_support()?;
    let event_loop = EventLoop::new()
        .map_err(|error| format!("failed to create overlay event loop: {error}"))?;
    let mut app = OverlayDemoApp::default();
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("overlay event loop failed: {error}"))
}

fn ensure_sidecar_support() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let has_display =
            env::var_os("DISPLAY").is_some() || env::var_os("WAYLAND_DISPLAY").is_some();
        if !has_display {
            return Err(String::from(
                "experimental sidecar overlay requires a desktop session with DISPLAY or WAYLAND_DISPLAY",
            ));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        return Err(String::from(
            "experimental sidecar overlay is only supported on macOS, Linux, or Windows desktop builds",
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
    build_overlay_demo_scene(
        &mut scene,
        &mut state.text_system,
        width,
        height,
        elapsed,
        OverlayPresentation::SidecarWindow,
    );

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
    presentation: OverlayPresentation,
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
        overlay_subtitle(presentation),
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
    paint_overlay_badges(left, presentation, &mut paint);

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
        Some(chart_body_note(presentation)),
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
        &overlay_feed_rows(time, presentation),
        &mut paint,
    );
}

fn overlay_subtitle(presentation: OverlayPresentation) -> &'static str {
    match presentation {
        OverlayPresentation::SidecarWindow => {
            "Ctrl+G can fall back to this sidecar from `probe tui`. Esc or close the window to dismiss it."
        }
        OverlayPresentation::TerminalInline => {
            "Ctrl+G can render this scene back into supported terminals. Press Enter to return to Probe."
        }
    }
}

fn chart_body_note(presentation: OverlayPresentation) -> &'static str {
    match presentation {
        OverlayPresentation::SidecarWindow => {
            "This is an experimental non-portable visual sidecar, not the default TUI renderer."
        }
        OverlayPresentation::TerminalInline => {
            "This is an experimental non-portable inline image lane driven by offscreen WGPUI capture."
        }
    }
}

fn overlay_feed_rows(time: f32, presentation: OverlayPresentation) -> [EventFeedRow<'static>; 4] {
    let pulse = ((time * 1.4).sin() + 1.0) * 0.5;
    let (launch_detail, renderer_detail, focus_detail) = match presentation {
        OverlayPresentation::SidecarWindow => (
            "The Probe TUI launched a WGPUI sidecar without surrendering terminal ownership.",
            "This lane is intentionally experimental and should remain capability-gated.",
            "Terminal input stays in Probe while the sidecar owns pointer and window focus.",
        ),
        OverlayPresentation::TerminalInline => (
            "The Probe TUI handed the terminal to an inline WGPUI image overlay for review.",
            "Pixels come from offscreen WGPUI capture plus a terminal image protocol, not ratatui cells.",
            "Probe returns after dismissal, re-enters the alternate screen, and redraws the TUI.",
        ),
    };

    [
        EventFeedRow {
            label: Cow::Borrowed("overlay_hotkey"),
            detail: Cow::Borrowed(launch_detail),
            color: badge_color(BadgeTone::Live),
        },
        EventFeedRow {
            label: Cow::Borrowed("renderer_mode"),
            detail: Cow::Borrowed(renderer_detail),
            color: provenance_color(ProvenanceTone::Evidence),
        },
        EventFeedRow {
            label: Cow::Borrowed("focus_model"),
            detail: Cow::Borrowed(focus_detail),
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

fn paint_overlay_badges(
    bounds: Bounds,
    presentation: OverlayPresentation,
    paint: &mut PaintContext,
) {
    let (lines, badges) = match presentation {
        OverlayPresentation::SidecarWindow => (
            [
                "Mode: experimental sidecar",
                "Host: Probe TUI + separate WGPUI window",
                "Dismiss: Esc or window close",
                "Purpose: richer visual telemetry proof, not terminal replacement",
            ],
            [
                ("EXPERIMENTAL", badge_color(BadgeTone::Warning)),
                ("TUI", badge_color(BadgeTone::TrackPgolf)),
                ("WGPUI", badge_color(BadgeTone::TrackHomegolf)),
                ("SIDECAR", badge_color(BadgeTone::TrackXtrain)),
            ],
        ),
        OverlayPresentation::TerminalInline => (
            [
                "Mode: experimental inline image",
                "Host: Probe TUI + iTerm2 image protocol",
                "Dismiss: Enter to return",
                "Purpose: richer visual telemetry proof inside the terminal",
            ],
            [
                ("EXPERIMENTAL", badge_color(BadgeTone::Warning)),
                ("TUI", badge_color(BadgeTone::TrackPgolf)),
                ("WGPUI", badge_color(BadgeTone::TrackHomegolf)),
                ("INLINE", badge_color(BadgeTone::TrackXtrain)),
            ],
        ),
    };

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

#[cfg(test)]
mod tests {
    use super::{
        TerminalImageProtocol, detect_terminal_image_protocol_with_inputs,
        iterm2_inline_image_escape,
    };

    #[test]
    fn iterm2_detection_requires_direct_interactive_session() {
        let protocol = detect_terminal_image_protocol_with_inputs(
            Some("iTerm.app"),
            Some("iTerm2"),
            true,
            true,
            false,
            false,
        );

        assert_eq!(protocol, Some(TerminalImageProtocol::ITerm2InlineImage));
    }

    #[test]
    fn iterm2_detection_refuses_tmux_passthrough_in_first_cut() {
        let protocol = detect_terminal_image_protocol_with_inputs(
            Some("iTerm.app"),
            Some("iTerm2"),
            true,
            true,
            true,
            false,
        );

        assert_eq!(protocol, None);
    }

    #[test]
    fn iterm2_escape_wraps_base64_payload() {
        let escape = iterm2_inline_image_escape(b"probe");

        assert!(escape.starts_with("\u{1b}]1337;File=inline=1"));
        assert!(escape.contains("cHJvYmU="));
        assert!(escape.ends_with('\u{7}'));
    }
}
