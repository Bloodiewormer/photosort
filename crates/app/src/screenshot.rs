use crate::demo::{DEFAULT_DEMO_COUNT, validate_count};
use crate::settings::Theme;
use eframe::egui::{self, ColorImage, ScreenshotCallback, ViewportCommand};
use fsguard::{DestPath, GuardError, LibraryRoot, ensure_writable};
use image::codecs::png::PngEncoder;
use image::imageops::{FilterType, resize};
use image::{ColorType, ImageEncoder, RgbaImage};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_WINDOW_SIZE: [u32; 2] = [1280, 820];
pub const DEFAULT_SCREENSHOT_DELAY_FRAMES: u32 = 3;
pub const DEFAULT_MAX_WAIT_FRAMES: u32 = 300;
pub const DEFAULT_MAX_CAPTURE_FRAMES: u32 = 60;

const MIN_WINDOW_WIDTH: u32 = 320;
const MIN_WINDOW_HEIGHT: u32 = 240;
const MAX_WINDOW_WIDTH: u32 = 7680;
const MAX_WINDOW_HEIGHT: u32 = 4320;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureView {
    #[default]
    Gallery,
    Settings,
    Welcome,
    Suggestions,
    Duplicates,
    Works,
    Persons,
    Tags,
    Errors,
    Export,
    Menu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub help: bool,
    pub self_test: bool,
    pub data_dir: Option<PathBuf>,
    pub demo: bool,
    pub demo_count: usize,
    pub screenshot: Option<PathBuf>,
    pub screenshot_delay_frames: u32,
    pub window_size: [u32; 2],
    pub theme: Option<Theme>,
    pub view: CaptureView,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            help: false,
            self_test: false,
            data_dir: None,
            demo: false,
            demo_count: DEFAULT_DEMO_COUNT,
            screenshot: None,
            screenshot_delay_frames: DEFAULT_SCREENSHOT_DELAY_FRAMES,
            window_size: DEFAULT_WINDOW_SIZE,
            theme: None,
            view: CaptureView::Gallery,
        }
    }
}

pub fn usage_text() -> &'static str {
    "Uso: photosort [--data-dir <carpeta>] [--self-test] [--demo] [--demo-count <N>] \
     [--screenshot <salida.png>] [--screenshot-delay-frames <N>] \
     [--window-size <Ancho>x<Alto>] [--theme dark|light] \
     [--view gallery|settings|welcome|suggestions|duplicates|works|personas|tags|errors|export|menu]"
}

pub fn parse_window_size(raw: &str) -> Result<[u32; 2], String> {
    let (w_str, h_str) = raw
        .split_once('x')
        .or_else(|| raw.split_once('X'))
        .ok_or_else(|| {
            format!(
                "Formato inválido para --window-size: '{raw}' (usa Ancho x Alto, p. ej. 1280x820)"
            )
        })?;
    let width: u32 = w_str
        .parse()
        .map_err(|_| format!("Ancho inválido en --window-size: '{w_str}'"))?;
    let height: u32 = h_str
        .parse()
        .map_err(|_| format!("Alto inválido en --window-size: '{h_str}'"))?;
    if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_WIDTH).contains(&width)
        || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_HEIGHT).contains(&height)
    {
        return Err(format!(
            "--window-size fuera de rango: {width}x{height} \
             (mínimo {MIN_WINDOW_WIDTH}x{MIN_WINDOW_HEIGHT}, máximo {MAX_WINDOW_WIDTH}x{MAX_WINDOW_HEIGHT})"
        ));
    }
    Ok([width, height])
}

pub fn parse_theme(raw: &str) -> Result<Theme, String> {
    if raw.eq_ignore_ascii_case("dark") {
        Ok(Theme::Dark)
    } else if raw.eq_ignore_ascii_case("light") {
        Ok(Theme::Light)
    } else {
        Err(format!(
            "Valor inválido para --theme: '{raw}' (usa 'dark' o 'light')"
        ))
    }
}

pub fn parse_view(raw: &str) -> Result<CaptureView, String> {
    if raw.eq_ignore_ascii_case("gallery") {
        Ok(CaptureView::Gallery)
    } else if raw.eq_ignore_ascii_case("settings") {
        Ok(CaptureView::Settings)
    } else if raw.eq_ignore_ascii_case("welcome") {
        Ok(CaptureView::Welcome)
    } else if raw.eq_ignore_ascii_case("suggestions") || raw.eq_ignore_ascii_case("review") {
        Ok(CaptureView::Suggestions)
    } else if raw.eq_ignore_ascii_case("duplicates") {
        Ok(CaptureView::Duplicates)
    } else if raw.eq_ignore_ascii_case("works") {
        Ok(CaptureView::Works)
    } else if raw.eq_ignore_ascii_case("personas") || raw.eq_ignore_ascii_case("persons") {
        Ok(CaptureView::Persons)
    } else if raw.eq_ignore_ascii_case("tags") {
        Ok(CaptureView::Tags)
    } else if raw.eq_ignore_ascii_case("errors") {
        Ok(CaptureView::Errors)
    } else if raw.eq_ignore_ascii_case("export") {
        Ok(CaptureView::Export)
    } else if raw.eq_ignore_ascii_case("menu") {
        Ok(CaptureView::Menu)
    } else {
        Err(format!(
            "Vista inválida para --view: '{raw}' (usa 'gallery', 'settings', 'welcome', 'suggestions', 'duplicates', 'works', 'personas', 'tags', 'errors', 'export' o 'menu')"
        ))
    }
}

pub fn parse_args<I, S>(args: I) -> Result<CliArgs, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    let mut parsed = CliArgs::default();

    while let Some(arg) = iter.next() {
        let Some(flag) = arg.to_str() else {
            return Err(format!("Argumento desconocido: {}", arg.to_string_lossy()));
        };
        match flag {
            "--help" | "-h" => {
                parsed.help = true;
            }
            "--self-test" => {
                parsed.self_test = true;
            }
            "--demo" => {
                parsed.demo = true;
            }
            "--data-dir" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta la ruta después de --data-dir".to_string())?;
                parsed.data_dir = Some(PathBuf::from(val));
            }
            "--demo-count" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta el número después de --demo-count".to_string())?;
                let text = val.to_string_lossy();
                let count: usize = text
                    .parse()
                    .map_err(|_| format!("Número inválido para --demo-count: '{text}'"))?;
                parsed.demo_count = validate_count(count)?;
            }
            "--screenshot" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta la ruta después de --screenshot".to_string())?;
                parsed.screenshot = Some(PathBuf::from(val));
            }
            "--screenshot-delay-frames" => {
                let val = iter.next().ok_or_else(|| {
                    "Falta el número después de --screenshot-delay-frames".to_string()
                })?;
                let text = val.to_string_lossy();
                let frames: u32 = text.parse().map_err(|_| {
                    format!("Número inválido para --screenshot-delay-frames: '{text}'")
                })?;
                parsed.screenshot_delay_frames = frames;
            }
            "--window-size" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta el tamaño después de --window-size".to_string())?;
                let text = val.to_string_lossy();
                parsed.window_size = parse_window_size(&text)?;
            }
            "--theme" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta el valor después de --theme".to_string())?;
                let text = val.to_string_lossy();
                parsed.theme = Some(parse_theme(&text)?);
            }
            "--view" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta la vista después de --view".to_string())?;
                let text = val.to_string_lossy();
                parsed.view = parse_view(&text)?;
            }
            other => {
                return Err(format!("Argumento desconocido: {other}"));
            }
        }
    }

    Ok(parsed)
}

/// Validates that the screenshot output path lies outside every registered library root.
pub fn guard_screenshot_path(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    ensure_writable(path, roots)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameSignals {
    pub has_entries: bool,
    pub has_selection: bool,
    pub thumbs_ready: bool,
    pub preview_ready: bool,
    pub export_open: bool,
    pub export_ready: bool,
    pub export_failed: bool,
    pub menu_open: bool,
    pub image_received: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureAction {
    SelectFirst,
    OpenExport,
    OpenMenu,
    WaitForWorkers,
    WaitFrame,
    RequestScreenshot,
    SaveAndClose,
    FailAndClose(String),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStep {
    None,
    SelectFirst,
    OpenExport,
    OpenMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    WaitingForReady { waited: u32 },
    Settling { remaining: u32 },
    CaptureRequested { waited: u32 },
    Done,
}

/// Pure frame-based state machine driving automated viewport screenshots without sleeps.
#[derive(Debug, Clone)]
pub struct CaptureState {
    view: CaptureView,
    delay_frames: u32,
    max_wait_frames: u32,
    max_capture_frames: u32,
    setup_triggered: bool,
    phase: Phase,
}

impl CaptureState {
    pub fn new(delay_frames: u32, max_wait_frames: u32, max_capture_frames: u32) -> Self {
        Self::for_view(
            CaptureView::Gallery,
            delay_frames,
            max_wait_frames,
            max_capture_frames,
        )
    }

    pub fn for_view(
        view: CaptureView,
        delay_frames: u32,
        max_wait_frames: u32,
        max_capture_frames: u32,
    ) -> Self {
        Self {
            view,
            delay_frames,
            max_wait_frames: max_wait_frames.max(1),
            max_capture_frames: max_capture_frames.max(1),
            setup_triggered: false,
            phase: Phase::WaitingForReady { waited: 0 },
        }
    }

    pub fn step(&mut self, signals: FrameSignals) -> CaptureAction {
        if self.phase == Phase::Done {
            return CaptureAction::Done;
        }

        if !self.setup_triggered && signals.has_entries {
            match self.view {
                CaptureView::Gallery if !signals.has_selection => {
                    self.setup_triggered = true;
                    return CaptureAction::SelectFirst;
                }
                CaptureView::Export if !signals.export_open => {
                    self.setup_triggered = true;
                    return CaptureAction::OpenExport;
                }
                CaptureView::Menu if !signals.menu_open => {
                    self.setup_triggered = true;
                    return CaptureAction::OpenMenu;
                }
                _ => {}
            }
        }

        match self.phase {
            Phase::WaitingForReady { waited } => {
                if self.view == CaptureView::Export && signals.export_failed {
                    self.phase = Phase::Done;
                    return CaptureAction::FailAndClose(
                        "No se pudo calcular la vista previa de exportación.".into(),
                    );
                }
                let ready = match self.view {
                    CaptureView::Gallery => {
                        !signals.has_entries || (signals.thumbs_ready && signals.preview_ready)
                    }
                    CaptureView::Menu => {
                        !signals.has_entries
                            || (signals.thumbs_ready && signals.preview_ready && signals.menu_open)
                    }
                    CaptureView::Suggestions | CaptureView::Duplicates | CaptureView::Errors => {
                        signals.thumbs_ready
                    }
                    CaptureView::Settings
                    | CaptureView::Welcome
                    | CaptureView::Works
                    | CaptureView::Persons
                    | CaptureView::Tags => true,
                    CaptureView::Export => {
                        !signals.has_entries || (signals.thumbs_ready && signals.export_ready)
                    }
                };
                if ready {
                    if self.delay_frames == 0 {
                        self.phase = Phase::CaptureRequested { waited: 0 };
                        CaptureAction::RequestScreenshot
                    } else {
                        self.phase = Phase::Settling {
                            remaining: self.delay_frames,
                        };
                        CaptureAction::WaitFrame
                    }
                } else {
                    let next_waited = waited.saturating_add(1);
                    if next_waited >= self.max_wait_frames {
                        self.phase = Phase::Done;
                        CaptureAction::FailAndClose(match self.view {
                            CaptureView::Gallery => {
                                "Tiempo de espera agotado esperando la carga de miniaturas y vista previa."
                                    .into()
                            }
                            CaptureView::Menu => {
                                "Tiempo de espera agotado esperando el menú contextual de la foto."
                                    .into()
                            }
                            CaptureView::Suggestions => {
                                "Tiempo de espera agotado esperando las miniaturas de Sugerencias."
                                    .into()
                            }
                            CaptureView::Duplicates => {
                                "Tiempo de espera agotado esperando las miniaturas de Duplicados."
                                    .into()
                            }
                            CaptureView::Settings => {
                                "Tiempo de espera agotado esperando Configuración.".into()
                            }
                            CaptureView::Welcome => {
                                "Tiempo de espera agotado esperando la pantalla inicial.".into()
                            }
                            CaptureView::Works => {
                                "Tiempo de espera agotado esperando Obras.".into()
                            }
                            CaptureView::Persons => {
                                "Tiempo de espera agotado esperando Personas.".into()
                            }
                            CaptureView::Tags => {
                                "Tiempo de espera agotado esperando Etiquetas.".into()
                            }
                            CaptureView::Errors => {
                                "Tiempo de espera agotado esperando las miniaturas de Errores."
                                    .into()
                            }
                            CaptureView::Export => {
                                "Tiempo de espera agotado esperando la vista previa del asistente de exportación."
                                    .into()
                            }
                        })
                    } else {
                        self.phase = Phase::WaitingForReady {
                            waited: next_waited,
                        };
                        CaptureAction::WaitForWorkers
                    }
                }
            }
            Phase::Settling { remaining } => {
                let next_remaining = remaining.saturating_sub(1);
                if next_remaining == 0 {
                    self.phase = Phase::CaptureRequested { waited: 0 };
                    CaptureAction::RequestScreenshot
                } else {
                    self.phase = Phase::Settling {
                        remaining: next_remaining,
                    };
                    CaptureAction::WaitFrame
                }
            }
            Phase::CaptureRequested { waited } => {
                if signals.image_received {
                    self.phase = Phase::Done;
                    return CaptureAction::SaveAndClose;
                }
                let next_waited = waited.saturating_add(1);
                if next_waited >= self.max_capture_frames {
                    self.phase = Phase::Done;
                    CaptureAction::FailAndClose(
                        "Tiempo de espera agotado esperando la captura del viewport.".into(),
                    )
                } else {
                    self.phase = Phase::CaptureRequested {
                        waited: next_waited,
                    };
                    CaptureAction::WaitFrame
                }
            }
            Phase::Done => CaptureAction::Done,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotSaved {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Clone)]
pub struct ScreenshotHandle {
    result: Arc<Mutex<Option<Result<ScreenshotSaved, String>>>>,
}

impl ScreenshotHandle {
    pub fn take_result(&self) -> Option<Result<ScreenshotSaved, String>> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[allow(clippy::disallowed_methods)] // `dir` is a fsguard DestPath verified outside library roots
fn create_dir_dest(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

#[allow(clippy::disallowed_methods)] // `dest` is a fsguard DestPath verified outside library roots
fn write_rgba_png_dest(
    dest: &DestPath,
    raw: &[u8],
    width: u32,
    height: u32,
) -> Result<u64, String> {
    let file = File::create(dest.as_path()).map_err(|e| e.to_string())?;
    let writer = BufWriter::new(file);
    PngEncoder::new(writer)
        .write_image(raw, width, height, ColorType::Rgba8.into())
        .map_err(|e| e.to_string())?;
    let bytes = fs::metadata(dest.as_path())
        .map(|m| m.len())
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Encodes an egui [`ColorImage`] as PNG to a guarded [`DestPath`]. If `target_size` is provided
/// and differs from the captured framebuffer (e.g. OS HiDPI scaling), resizes cleanly to match.
pub fn write_screenshot_png(
    dest: &DestPath,
    roots: &[LibraryRoot],
    image: &ColorImage,
    target_size: Option<[u32; 2]>,
) -> Result<ScreenshotSaved, String> {
    if let Some(parent) = dest.as_path().parent()
        && !parent.as_os_str().is_empty()
    {
        let parent_dest = ensure_writable(parent, roots).map_err(|e| e.to_string())?;
        create_dir_dest(&parent_dest).map_err(|e| e.to_string())?;
    }

    let cap_w = u32::try_from(image.width()).map_err(|_| "ancho de captura inválido")?;
    let cap_h = u32::try_from(image.height()).map_err(|_| "alto de captura inválido")?;
    if cap_w == 0 || cap_h == 0 {
        return Err("La captura del viewport está vacía (0x0).".into());
    }

    let (final_w, final_h, bytes) = if let Some([tw, th]) = target_size
        && (cap_w != tw || cap_h != th)
    {
        let rgba = RgbaImage::from_raw(cap_w, cap_h, image.as_raw().to_vec())
            .ok_or_else(|| "Búfer de captura RGBA inválido.".to_string())?;
        let resized = resize(&rgba, tw, th, FilterType::Lanczos3);
        let written = write_rgba_png_dest(dest, resized.as_raw(), tw, th)?;
        (tw, th, written)
    } else {
        let written = write_rgba_png_dest(dest, image.as_raw(), cap_w, cap_h)?;
        (cap_w, cap_h, written)
    };

    Ok(ScreenshotSaved {
        path: dest.as_path().to_path_buf(),
        width: final_w,
        height: final_h,
        bytes,
    })
}

/// Drives automated screenshot capture inside `eframe::App::ui` and reports the outcome to `main`.
pub struct ScreenshotController {
    state: CaptureState,
    dest: DestPath,
    roots: Vec<LibraryRoot>,
    target_size: [u32; 2],
    captured: Arc<Mutex<Option<Arc<ColorImage>>>>,
    outcome: Arc<Mutex<Option<Result<ScreenshotSaved, String>>>>,
}

impl ScreenshotController {
    pub fn new(
        dest: DestPath,
        roots: Vec<LibraryRoot>,
        view: CaptureView,
        delay_frames: u32,
        target_size: [u32; 2],
    ) -> (Self, ScreenshotHandle) {
        let outcome = Arc::new(Mutex::new(None));
        let handle = ScreenshotHandle {
            result: Arc::clone(&outcome),
        };
        let controller = Self {
            state: CaptureState::for_view(
                view,
                delay_frames,
                DEFAULT_MAX_WAIT_FRAMES,
                DEFAULT_MAX_CAPTURE_FRAMES,
            ),
            dest,
            roots,
            target_size,
            captured: Arc::new(Mutex::new(None)),
            outcome,
        };
        (controller, handle)
    }

    /// Advances the screenshot state machine for the current frame.
    /// Returns a [`CaptureStep`] instructing the caller to perform any view-specific setup action.
    pub fn step(&mut self, ctx: &egui::Context, mut signals: FrameSignals) -> CaptureStep {
        signals.image_received = self
            .captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();

        let action = self.state.step(signals);

        match action {
            CaptureAction::SelectFirst => {
                ctx.request_repaint();
                CaptureStep::SelectFirst
            }
            CaptureAction::OpenExport => {
                ctx.request_repaint();
                CaptureStep::OpenExport
            }
            CaptureAction::OpenMenu => {
                ctx.request_repaint();
                CaptureStep::OpenMenu
            }
            CaptureAction::WaitForWorkers => {
                ctx.request_repaint_after(Duration::from_millis(25));
                CaptureStep::None
            }
            CaptureAction::WaitFrame => {
                ctx.request_repaint();
                CaptureStep::None
            }
            CaptureAction::RequestScreenshot => {
                let slot = Arc::clone(&self.captured);
                let ctx_cb = ctx.clone();
                let cb = ScreenshotCallback::new(move |img| {
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(img);
                    ctx_cb.request_repaint();
                });
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(cb));
                ctx.request_repaint();
                CaptureStep::None
            }
            CaptureAction::SaveAndClose => {
                let img = self
                    .captured
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                let res = match img {
                    Some(color_img) => write_screenshot_png(
                        &self.dest,
                        &self.roots,
                        &color_img,
                        Some(self.target_size),
                    ),
                    None => Err("No se recibió la imagen de captura del viewport.".into()),
                };
                *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
                ctx.send_viewport_cmd(ViewportCommand::Close);
                CaptureStep::None
            }
            CaptureAction::FailAndClose(msg) => {
                *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(msg));
                ctx.send_viewport_cmd(ViewportCommand::Close);
                CaptureStep::None
            }
            CaptureAction::Done => CaptureStep::None,
        }
    }
}
