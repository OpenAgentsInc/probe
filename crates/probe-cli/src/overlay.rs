use std::borrow::Cow;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::ValueEnum;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self as terminal_event, Event as TerminalEvent, KeyCode as TerminalKeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, disable_raw_mode, enable_raw_mode, size as terminal_size,
};
use image::codecs::gif::{GifEncoder, Repeat as GifRepeat};
use image::{Delay as ImageDelay, Frame as ImageFrame, ImageFormat};
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
const TERMINAL_CELL_WIDTH_PX: u32 = 12;
const TERMINAL_CELL_HEIGHT_PX: u32 = 24;
const TERMINAL_OVERLAY_GIF_FRAME_COUNT: usize = 10;
const TERMINAL_OVERLAY_GIF_FRAME_DELAY_MS: u32 = 110;
const TERMINAL_OVERLAY_GIF_TIME_STEP: f32 = 0.32;
const ITERM2_FILE_PART_CHARS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OverlayTarget {
    Auto,
    Terminal,
    Sidecar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayPresentation {
    SidecarWindow,
    TerminalInline(TerminalViewport),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalViewport {
    cols: u16,
    rows: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayLaunchContext {
    StandaloneCli,
    EmbeddedInTui,
}

pub fn run_overlay_demo(target: OverlayTarget, from_tui_handoff: bool) -> Result<(), String> {
    let launch_context = if from_tui_handoff {
        OverlayLaunchContext::EmbeddedInTui
    } else {
        OverlayLaunchContext::StandaloneCli
    };
    run_overlay_demo_inner(target, launch_context)
}

fn run_overlay_demo_inner(
    target: OverlayTarget,
    launch_context: OverlayLaunchContext,
) -> Result<(), String> {
    match resolve_overlay_target(target)? {
        ResolvedOverlayTarget::Sidecar => run_sidecar_overlay_demo(),
        ResolvedOverlayTarget::Terminal(protocol) => {
            run_terminal_overlay_demo(protocol, launch_context)
        }
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

fn run_terminal_overlay_demo(
    protocol: TerminalImageProtocol,
    launch_context: OverlayLaunchContext,
) -> Result<(), String> {
    let mut stdout = io::stdout();
    let viewport = terminal_viewport()?;
    prepare_terminal_overlay_surface(&mut stdout, launch_context)?;
    let run_result = run_live_terminal_overlay_demo(&mut stdout, protocol, viewport);
    let cleanup_result = cleanup_terminal_overlay_surface(&mut stdout, launch_context);
    run_result.and(cleanup_result)
}

fn write_terminal_image_payload(
    stdout: &mut io::Stdout,
    protocol: TerminalImageProtocol,
    viewport: TerminalViewport,
    image_bytes: &[u8],
) -> Result<(), String> {
    match protocol {
        TerminalImageProtocol::ITerm2InlineImage => {
            write_iterm2_inline_image_payload(stdout, viewport, image_bytes)
        }
    }
}

fn write_iterm2_inline_image_payload(
    stdout: &mut io::Stdout,
    viewport: TerminalViewport,
    image_bytes: &[u8],
) -> Result<(), String> {
    let chunks = iterm2_multipart_inline_image_chunks(viewport, image_bytes, "image/gif");
    for chunk in chunks {
        stdout.write_all(chunk.as_bytes()).map_err(|error| {
            format!("failed to write terminal overlay multipart chunk: {error}")
        })?;
    }
    Ok(())
}

fn iterm2_multipart_inline_image_chunks(
    viewport: TerminalViewport,
    image_bytes: &[u8],
    content_type: &str,
) -> Vec<String> {
    let image_rows = viewport.rows.saturating_sub(1).max(1);
    let payload = STANDARD.encode(image_bytes);
    let mut chunks = Vec::with_capacity(payload.len() / ITERM2_FILE_PART_CHARS + 2);
    chunks.push(format!(
        "\u{1b}]1337;MultipartFile=inline=1;size={};width={};height={};preserveAspectRatio=0;type={content_type}\u{7}",
        image_bytes.len(),
        viewport.cols.max(1),
        image_rows
    ));
    for chunk in payload.as_bytes().chunks(ITERM2_FILE_PART_CHARS) {
        chunks.push(format!(
            "\u{1b}]1337;FilePart={}\u{7}",
            std::str::from_utf8(chunk).expect("base64 payload is always valid ASCII")
        ));
    }
    chunks.push(String::from("\u{1b}]1337;FileEnd\u{7}\n"));
    chunks
}

fn render_overlay_demo_png(
    presentation: OverlayPresentation,
    time: f32,
) -> Result<Vec<u8>, String> {
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

    let capture_size = overlay_capture_size(presentation);
    let mut scene = Scene::new();
    let mut text_system = TextSystem::new(1.0);
    build_overlay_demo_scene(
        &mut scene,
        &mut text_system,
        capture_size.width as f32,
        capture_size.height as f32,
        time,
        presentation,
    );

    let mut request = CaptureRequest::new(
        CaptureTarget::AdHoc {
            name: String::from("probe_overlay_demo"),
        },
        capture_size.width,
        capture_size.height,
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

fn terminal_viewport() -> Result<TerminalViewport, String> {
    let (cols, rows) =
        terminal_size().map_err(|error| format!("failed to read terminal size: {error}"))?;
    Ok(TerminalViewport {
        cols: cols.max(40),
        rows: rows.max(16),
    })
}

fn overlay_capture_size(presentation: OverlayPresentation) -> CaptureSize {
    match presentation {
        OverlayPresentation::SidecarWindow => CaptureSize {
            width: OVERLAY_WIDTH,
            height: OVERLAY_HEIGHT,
        },
        OverlayPresentation::TerminalInline(viewport) => CaptureSize {
            width: u32::from(viewport.cols) * TERMINAL_CELL_WIDTH_PX,
            height: u32::from(viewport.rows.saturating_sub(1).max(1)) * TERMINAL_CELL_HEIGHT_PX,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaptureSize {
    width: u32,
    height: u32,
}

fn run_live_terminal_overlay_demo(
    stdout: &mut io::Stdout,
    protocol: TerminalImageProtocol,
    viewport: TerminalViewport,
) -> Result<(), String> {
    paint_terminal_overlay_loading_state(stdout, viewport)?;
    let gif_bytes = render_overlay_demo_gif(viewport)?;
    display_terminal_overlay_asset(stdout, protocol, viewport, gif_bytes.as_slice())?;
    wait_for_terminal_overlay_dismissal()
}

fn prepare_terminal_overlay_surface(
    stdout: &mut io::Stdout,
    launch_context: OverlayLaunchContext,
) -> Result<(), String> {
    if matches!(launch_context, OverlayLaunchContext::StandaloneCli) {
        enable_raw_mode()
            .map_err(|error| format!("failed to enable raw mode for terminal overlay: {error}"))?;
    }
    execute!(stdout, Hide, Clear(ClearType::All), MoveTo(0, 0))
        .map_err(|error| format!("failed to prepare terminal overlay surface: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush terminal overlay setup: {error}"))
}

fn paint_terminal_overlay_loading_state(
    stdout: &mut io::Stdout,
    viewport: TerminalViewport,
) -> Result<(), String> {
    let loading_copy = "Rendering animated WGPUI overlay...";
    let x = viewport
        .cols
        .saturating_sub(loading_copy.len() as u16)
        .checked_div(2)
        .unwrap_or(0);
    let y = viewport.rows.saturating_div(2);
    execute!(stdout, MoveTo(x, y))
        .map_err(|error| format!("failed to position terminal overlay loading copy: {error}"))?;
    write!(stdout, "{loading_copy}")
        .map_err(|error| format!("failed to write terminal overlay loading copy: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush terminal overlay loading copy: {error}"))
}

fn render_overlay_demo_gif(viewport: TerminalViewport) -> Result<Vec<u8>, String> {
    let capture_size = overlay_capture_size(OverlayPresentation::TerminalInline(viewport));
    let gif_width = u16::try_from(capture_size.width).map_err(|_| {
        format!(
            "terminal overlay width {} exceeds GIF encoder limits",
            capture_size.width
        )
    })?;
    let gif_height = u16::try_from(capture_size.height).map_err(|_| {
        format!(
            "terminal overlay height {} exceeds GIF encoder limits",
            capture_size.height
        )
    })?;
    let mut gif_bytes = Vec::new();
    let mut encoder = GifEncoder::new(&mut gif_bytes);
    encoder
        .set_repeat(GifRepeat::Infinite)
        .map_err(|error| format!("failed to configure terminal overlay GIF repeat: {error}"))?;

    for frame_index in 0..TERMINAL_OVERLAY_GIF_FRAME_COUNT {
        let time = frame_index as f32 * TERMINAL_OVERLAY_GIF_TIME_STEP;
        let png_bytes =
            render_overlay_demo_png(OverlayPresentation::TerminalInline(viewport), time)?;
        let frame_image = image::load_from_memory_with_format(&png_bytes, ImageFormat::Png)
            .map_err(|error| format!("failed to decode terminal overlay frame PNG: {error}"))?
            .into_rgba8();
        if frame_image.width() != u32::from(gif_width)
            || frame_image.height() != u32::from(gif_height)
        {
            return Err(format!(
                "terminal overlay frame size drifted from {}x{} to {}x{}",
                gif_width,
                gif_height,
                frame_image.width(),
                frame_image.height()
            ));
        }
        let frame = ImageFrame::from_parts(
            frame_image,
            0,
            0,
            ImageDelay::from_numer_denom_ms(TERMINAL_OVERLAY_GIF_FRAME_DELAY_MS, 1),
        );
        encoder
            .encode_frame(frame)
            .map_err(|error| format!("failed to encode terminal overlay GIF frame: {error}"))?;
    }

    drop(encoder);
    Ok(gif_bytes)
}

fn display_terminal_overlay_asset(
    stdout: &mut io::Stdout,
    protocol: TerminalImageProtocol,
    viewport: TerminalViewport,
    asset_bytes: &[u8],
) -> Result<(), String> {
    execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|error| format!("failed to prepare terminal overlay surface: {error}"))?;
    write_terminal_image_payload(stdout, protocol, viewport, asset_bytes)?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush terminal overlay asset: {error}"))
}

fn wait_for_terminal_overlay_dismissal() -> Result<(), String> {
    loop {
        if terminal_overlay_key_should_dismiss(
            terminal_event::read()
                .map_err(|error| format!("failed to read terminal overlay event: {error}"))?,
        ) {
            return Ok(());
        }
    }
}

fn terminal_overlay_key_should_dismiss(event: TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::Key(key_event)
            if key_event.kind == KeyEventKind::Press
                && matches!(
                    key_event.code,
                    TerminalKeyCode::Enter | TerminalKeyCode::Esc | TerminalKeyCode::Char('q')
                )
    )
}

fn cleanup_terminal_overlay_surface(
    stdout: &mut io::Stdout,
    launch_context: OverlayLaunchContext,
) -> Result<(), String> {
    let cleanup_result = match launch_context {
        OverlayLaunchContext::StandaloneCli => {
            execute!(stdout, Show, Clear(ClearType::All), MoveTo(0, 0))
                .map_err(|error| format!("failed to clear terminal overlay surface: {error}"))
        }
        OverlayLaunchContext::EmbeddedInTui => {
            execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))
                .map_err(|error| format!("failed to clear terminal overlay surface: {error}"))
        }
    };
    let raw_mode_result = if matches!(launch_context, OverlayLaunchContext::StandaloneCli) {
        disable_raw_mode()
            .map_err(|error| format!("failed to disable raw mode after terminal overlay: {error}"))
    } else {
        Ok(())
    };
    let flush_result = stdout
        .flush()
        .map_err(|error| format!("failed to flush terminal overlay cleanup: {error}"));
    cleanup_result.and(raw_mode_result).and(flush_result)
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
    paint_terminal_overlay_frame(root, phase, presentation, &mut paint);
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

    if matches!(presentation, OverlayPresentation::TerminalInline(_)) {
        paint_terminal_overlay_footer(root, &mut paint);
    }
}

fn overlay_subtitle(presentation: OverlayPresentation) -> &'static str {
    match presentation {
        OverlayPresentation::SidecarWindow => {
            "Ctrl+G can fall back to this sidecar from `probe tui`. Esc or close the window to dismiss it."
        }
        OverlayPresentation::TerminalInline(_) => {
            "Inline terminal overlay refreshed in-place inside Probe's active screen."
        }
    }
}

fn chart_body_note(presentation: OverlayPresentation) -> &'static str {
    match presentation {
        OverlayPresentation::SidecarWindow => {
            "This is an experimental non-portable visual sidecar, not the default TUI renderer."
        }
        OverlayPresentation::TerminalInline(_) => {
            "Offscreen WGPUI frames are streamed back through the host terminal image protocol."
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
        OverlayPresentation::TerminalInline(_) => (
            "Probe keeps the alternate screen live and swaps inline WGPUI frames in place.",
            "Pixels come from repeated offscreen WGPUI capture plus the terminal image protocol, not ratatui cells.",
            "Dismissal returns to the same Probe TUI session without dropping to the shell.",
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
        OverlayPresentation::TerminalInline(_) => (
            [
                "Mode: live inline overlay",
                "Host: Probe alt-screen + iTerm2 image protocol",
                "Dismiss: Enter / Esc / q",
                "Purpose: richer WGPUI telemetry proof inside the terminal",
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

fn paint_terminal_overlay_frame(
    bounds: Bounds,
    phase: f32,
    presentation: OverlayPresentation,
    paint: &mut PaintContext,
) {
    if !matches!(presentation, OverlayPresentation::TerminalInline(_)) {
        return;
    }

    let glow = Bounds::new(
        bounds.origin.x + 24.0,
        bounds.origin.y + 24.0,
        bounds.size.width - 48.0,
        bounds.size.height - 48.0,
    );
    paint.scene.draw_quad(
        Quad::new(glow)
            .with_background(viz_theme::series::LOSS.with_alpha(0.04 + phase * 0.02))
            .with_border(viz_theme::series::LOSS.with_alpha(0.28), 1.0)
            .with_corner_radius(14.0),
    );
}

fn paint_terminal_overlay_footer(bounds: Bounds, paint: &mut PaintContext) {
    let footer = Bounds::new(
        bounds.origin.x + 24.0,
        bounds.max_y() - 64.0,
        bounds.size.width - 48.0,
        40.0,
    );
    paint.scene.draw_quad(
        Quad::new(footer)
            .with_background(theme::bg::ELEVATED.with_alpha(0.18))
            .with_border(viz_theme::series::LOSS.with_alpha(0.42), 1.0)
            .with_corner_radius(8.0),
    );
    paint.scene.draw_text(paint.text.layout_mono(
        "ESC / ENTER / Q  back to Probe",
        Point::new(footer.origin.x + 14.0, footer.origin.y + 12.0),
        11.0,
        viz_theme::series::LOSS.with_alpha(0.94),
    ));
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
        ITERM2_FILE_PART_CHARS, TerminalImageProtocol, TerminalViewport,
        detect_terminal_image_protocol_with_inputs, iterm2_multipart_inline_image_chunks,
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
        let chunks = iterm2_multipart_inline_image_chunks(
            TerminalViewport {
                cols: 120,
                rows: 40,
            },
            &vec![b'p'; ITERM2_FILE_PART_CHARS + 17],
            "image/gif",
        );

        assert!(chunks[0].starts_with("\u{1b}]1337;MultipartFile=inline=1"));
        assert!(chunks[0].contains("size=4113"));
        assert!(chunks[0].contains("width=120"));
        assert!(chunks[0].contains("height=39"));
        assert!(chunks[0].contains("type=image/gif"));
        assert_eq!(chunks.len(), 4);
        assert!(chunks[1].starts_with("\u{1b}]1337;FilePart="));
        assert!(chunks[2].starts_with("\u{1b}]1337;FilePart="));
        assert_eq!(chunks[3], "\u{1b}]1337;FileEnd\u{7}\n");
    }
}
