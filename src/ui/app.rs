//! Application state and root layout.
//!
//! The window frame is the same one the egui front end drew: a 64px top bar on
//! `surface`, a 52px status bar on `surface`, and a scrollable central panel on
//! `app_bg` with 24×18 padding. What moved is the toast: it used to float over
//! the page at the top-right and now sits inside the status bar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, Sender},
};
use std::time::{Duration, Instant};

use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{Root, Theme as KitTheme, ThemeMode};
use gpui_kit::*;

use crate::config::{AppConfig, ExportConfig, MonitorConfig};
use crate::monitor::{MonitorHandle, MonitorState};
use crate::ui::fonts::FontSlot;
use crate::ui::locale::{self, Lang};
use crate::ui::{components, fonts, theme};

pub const PATTERN_QUICK: &str = "regex:(?m)^(C\\d{3,})$";
pub const PATTERN_FULL: &str = "regex:编号[：:]\\s*(C\\d+)";
const RELEASES_API_URL: &str = "https://api.github.com/repos/ref42/seex/releases/latest";
const RELEASES_WEB_URL: &str = "https://github.com/ref42/seex/releases/latest";
/// How long after start-up the first background update check runs, and how
/// often it asks again. The check used to happen only when someone pressed the
/// button on the More page, so a release could sit unnoticed indefinitely.
const UPDATE_CHECK_DELAY: Duration = Duration::from_secs(5);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Monitor,
    History,
    Export,
    More,
    Font,
}

/// Every text box the pages own, created the first time a page asks for it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputSlot {
    MatchedPath,
    ExportPath,
    LibraryName,
    Parallel,
    StrokeHex,
    FillHex,
    FontSearch,
}

pub enum ManualImportStatus {
    Imported(usize),
    NoIds,
}

/// The two schematic colours the Export settings can pick, named so the picker
/// registry, the hex boxes and the exporter all agree on which is which.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ColorSlot {
    Stroke,
    Fill,
}

impl ColorSlot {
    pub const ALL: [ColorSlot; 2] = [ColorSlot::Stroke, ColorSlot::Fill];

    fn index(self) -> usize {
        match self {
            ColorSlot::Stroke => 0,
            ColorSlot::Fill => 1,
        }
    }

    /// The box beside the swatch, which shows and edits the same colour.
    fn input_slot(self) -> InputSlot {
        match self {
            ColorSlot::Stroke => InputSlot::StrokeHex,
            ColorSlot::Fill => InputSlot::FillHex,
        }
    }

    /// The colour the slot carries, or `None` for the fill slot's "no colour".
    fn color(self, ui: &SeexUi) -> Option<[u8; 3]> {
        match self {
            ColorSlot::Stroke => Some(ui.schematic_stroke_color),
            ColorSlot::Fill => ui.schematic_fill_color,
        }
    }
}

#[derive(Clone, Default)]
pub struct UpdateState {
    pub checking: bool,
    pub checked: bool,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub error: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub html_url: String,
}

#[derive(Clone, Copy)]
pub enum FileDialogKind {
    SaveCsv,
    PickFolder,
}

pub struct FileDialogResult {
    pub kind: FileDialogKind,
    pub path: Option<PathBuf>,
}

pub enum ToastMessage {
    Success(String),
    Error(String),
    Info(String),
}

#[derive(Default)]
pub struct ToastState {
    pub message: Option<ToastMessage>,
    pub show_until: Option<Instant>,
}

/// Where a hover hint is shown. Hints that explain a row of controls are drawn
/// under that row; the rest fall back to the status bar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HintArea {
    StatusBar,
    ExportToggles,
}

pub struct SeexUi {
    pub state: Arc<Mutex<MonitorState>>,
    _monitor: MonitorHandle,
    /// The donation code — one image covering both payment apps — decoded once
    /// from the embedded PNG.
    pub donation_code: Arc<Image>,
    pub donation_aspect: f32,
    pub page: Page,
    pub pin_on_top: bool,
    pub show_matched: bool,
    pub show_history: bool,
    pub match_quick: bool,
    pub match_full: bool,
    pub clear_confirm: bool,
    last_theme_check: Instant,
    pub update_state: Arc<Mutex<UpdateState>>,
    /// When the next background update check falls due.
    next_update_check: Instant,
    pub show_donate: bool,
    pub theme_preset: String,
    pub schematic_stroke_color: [u8; 3],
    /// `None` is the "no colour" fill: the symbol body keeps its outline and is
    /// exported with a transparent area.
    pub schematic_fill_color: Option<[u8; 3]>,
    pub schematic_stroke_hex: String,
    pub schematic_fill_hex: String,
    /// One picker per schematic colour, created the first time the Export page
    /// draws the swatch, so the constructor need not ask for a `Window`.
    color_pickers: HashMap<ColorSlot, Entity<ColorPickerState>>,
    /// Set when a hex box edits a colour. The picker cannot be told from there
    /// (no `Window`), so the next render pushes the value into its sliders.
    color_sync: [bool; 2],
    pub manual_import_text: String,
    pub manual_import_status: Option<ManualImportStatus>,
    /// The manual-import box on the Monitor page.
    pub import_input: Entity<TextareaState>,
    /// One text box per [`InputSlot`], created the first time a page asks for
    /// it so the constructor need not know every field up front.
    inputs: HashMap<InputSlot, Entity<InputState>>,
    _input_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    pub file_dialog_tx: Sender<FileDialogResult>,
    pub file_dialog_rx: Receiver<FileDialogResult>,
    pub file_dialog_running: bool,
    pub file_dialog_start_time: Option<Instant>,
    pub toast_state: ToastState,
    /// What the control under the pointer does, and where to show it.
    pub hover_hint: Option<(HintArea, SharedString)>,
    config_dirty: bool,
    config_save_time: Option<Instant>,
    /// Roman-script face chosen on the Font page; `None` means the platform UI
    /// face picked at start-up.
    pub font_latin_choice: Option<String>,
    /// CJK face, which GPUI uses as the fallback for Han characters.
    pub font_han_choice: Option<String>,
    /// The family every text run starts from.
    pub font_latin: SharedString,
    /// Consulted only for characters the Latin family cannot draw.
    pub han_fallbacks: FontFallbacks,
    /// Every installed family, arranged for the two pickers.
    pub latin_choices: Vec<String>,
    pub han_choices: Vec<String>,
    /// Which picker, if either, is open on the Font page.
    pub font_picker: Option<FontSlot>,
    /// Every family the system reports, read once at start-up.
    pub font_names: Vec<String>,
    /// The face used for timestamps and other monospaced text.
    pub mono_family: SharedString,
}

pub fn app_title() -> String {
    "SeEx: seek and export".to_string()
}

/// Keeps the window above the others, the way the old `PIN` button did.
///
/// GPUI exposes no window-level API, so this reaches for the platform call on
/// the window it finds by title, after checking that the window really belongs
/// to this process.
#[cfg(windows)]
pub fn set_always_on_top(on_top: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, SetWindowPos,
    };

    let title: Vec<u16> = app_title()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd.is_null() {
        return;
    }

    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != std::process::id() {
        return;
    }

    let after = if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe { SetWindowPos(hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE) };
}

#[cfg(not(windows))]
pub fn set_always_on_top(_on_top: bool) {}

/// Rounds the window's corners the way Windows 11 and macOS do, by asking the
/// compositor for the rounded preference. On older Windows the request is
/// ignored, which leaves the square frame rather than failing.
#[cfg(windows)]
pub fn round_window_corners() {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

    /// `DWMWA_WINDOW_CORNER_PREFERENCE`, with `DWMWCP_ROUND` as its value.
    const CORNER_PREFERENCE: u32 = 33;
    const ROUND: u32 = 2;

    let title: Vec<u16> = app_title()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(std::ptr::null(), title.as_ptr())
    };
    if hwnd.is_null() {
        return;
    }

    let mut pid = 0u32;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &mut pid)
    };
    if pid != std::process::id() {
        return;
    }

    let round = ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            CORNER_PREFERENCE,
            std::ptr::addr_of!(round).cast(),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn round_window_corners() {}

/// Opens the window and keeps it alive until the user closes it.
pub fn run() {
    set_windows_app_id();

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);

        let dark = theme::detect_dark_mode();
        theme::set_dark_mode(dark);
        KitTheme::change(
            if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );

        let font_names = cx.text_system().all_font_names();
        // The kit draws its own widgets — tooltips, text boxes — from its theme
        // font, so the saved family has to be pushed there at start-up too, not
        // only when the user picks one on the Font page.
        let saved = AppConfig::load().latin_font;
        let family: SharedString = saved
            .unwrap_or_else(|| system_font(&font_names).to_string())
            .into();
        cx.global_mut::<KitTheme>().font_family = family.clone();
        fonts::remember(fonts::ui_font(
            family,
            FontFallbacks::from_fonts(fonts::han_chain(AppConfig::load().han_font.as_deref())),
        ));

        let bounds = Bounds::new(point(px(120.0), px(80.0)), size(px(1080.0), px(900.0)));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1080.0), px(720.0))),
            ..Default::default()
        };

        let opened = cx.open_window(options, |window, cx| {
            window.set_window_title(&app_title());
            let view = cx.new(|cx| SeexUi::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx).bg(theme::app_bg()))
        });
        if let Err(error) = opened {
            eprintln!("SeEx: could not open its window: {error}");
        } else {
            // egui opened always-on-top; keep that as the starting state.
            set_always_on_top(true);
            round_window_corners();
        }

        // Ask again once the window is mapped: some compositors ignore the
        // preference when it arrives with the very first frame.
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            round_window_corners();
        })
        .detach();
    });
}

/// The platform's own UI face, in the order Windows itself prefers.
pub fn system_font(installed: &[String]) -> &'static str {
    [
        "Segoe UI Variable Text",
        "Segoe UI Variable Display",
        "Segoe UI Variable",
        "Segoe UI",
        "Microsoft YaHei UI",
        "MiSans",
        "Noto Sans",
        "Arial",
    ]
    .into_iter()
    .find(|name| installed.iter().any(|font| font == name))
    .unwrap_or("Segoe UI")
}

/// The monospaced counterpart, for timestamps.
pub fn mono_font(installed: &[String]) -> &'static str {
    ["Cascadia Mono", "Consolas", "JetBrains Mono", "Courier New"]
        .into_iter()
        .find(|name| installed.iter().any(|font| font == name))
        .unwrap_or_else(|| system_font(installed))
}

impl SeexUi {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        let config = AppConfig::load();
        theme::set_theme_index(theme::theme_index(&config.theme));

        let mut initial = MonitorState::new();
        apply_config_to_state(&config, &mut initial);
        initial.set_keyword(build_keyword(true, true));

        let state = Arc::new(Mutex::new(initial));
        let monitor = MonitorHandle::spawn(Arc::clone(&state));
        let (file_dialog_tx, file_dialog_rx) = mpsc::channel();

        let font_names = cx.text_system().all_font_names();
        let mono_family: SharedString = mono_font(&font_names).into();
        let latin_choices = fonts::choices(
            &font_names,
            fonts::FontSlot::Latin,
            config.latin_font.as_deref(),
        );
        let han_choices = fonts::choices(
            &font_names,
            fonts::FontSlot::Han,
            config.han_font.as_deref(),
        );
        let font_latin: SharedString = config
            .latin_font
            .clone()
            .unwrap_or_else(|| system_font(&font_names).to_string())
            .into();
        let han_fallbacks = FontFallbacks::from_fonts(fonts::han_chain(config.han_font.as_deref()));
        locale::set_lang(if config.ui_language.eq_ignore_ascii_case("chinese") {
            Lang::Zh
        } else {
            Lang::En
        });

        let import_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(locale::t("Example: C2040, C42", "示例：C2040, C42"))
        });

        let (donation_code, donation_aspect) = load_donation(
            include_bytes!("../../assets/donate/donation_code.png").to_vec(),
            ImageFormat::Png,
        );

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.tick(cx);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            state,
            _monitor: monitor,
            donation_code,
            donation_aspect,
            page: Page::Monitor,
            pin_on_top: true,
            show_matched: true,
            show_history: true,
            match_quick: true,
            match_full: true,
            clear_confirm: false,
            last_theme_check: Instant::now(),
            update_state: Arc::new(Mutex::new(UpdateState::default())),
            next_update_check: Instant::now() + UPDATE_CHECK_DELAY,
            show_donate: config.show_donate,
            theme_preset: theme::normalize_theme_name(&config.theme),
            schematic_stroke_color: config.export.schematic_stroke_color,
            schematic_fill_color: config.export.schematic_fill_color,
            schematic_stroke_hex: color_to_hex(config.export.schematic_stroke_color),
            schematic_fill_hex: config
                .export
                .schematic_fill_color
                .map(color_to_hex)
                .unwrap_or_default(),
            color_pickers: HashMap::new(),
            color_sync: [false; 2],
            manual_import_text: String::new(),
            manual_import_status: None,
            import_input,
            inputs: HashMap::new(),
            _input_subscriptions: Vec::new(),
            _subscriptions: Vec::new(),
            file_dialog_tx,
            file_dialog_rx,
            file_dialog_running: false,
            file_dialog_start_time: None,
            toast_state: ToastState::default(),
            hover_hint: None,
            config_dirty: false,
            config_save_time: None,
            font_latin_choice: config.latin_font.clone(),
            font_han_choice: config.han_font.clone(),
            font_latin,
            han_fallbacks,
            latin_choices,
            han_choices,
            font_picker: None,
            font_names,
            mono_family,
        }
    }

    /// The text box for `slot`, created on first use so each page can own its
    /// own fields without the constructor listing them all.
    pub fn input(
        &mut self,
        slot: InputSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(entity) = self.inputs.get(&slot) {
            return entity.clone();
        }
        let (placeholder, value) = self.input_seed(slot);
        let entity = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(value)
        });
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.input_changed(slot, cx);
                }
            },
        );
        self._input_subscriptions.push(subscription);
        self.inputs.insert(slot, entity.clone());
        entity
    }

    /// The current contents of `slot`'s box.
    pub fn input_value(&self, slot: InputSlot, cx: &App) -> String {
        self.inputs
            .get(&slot)
            .map(|entity| entity.read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// The colour picker behind one of the two swatches, created on first use
    /// for the same reason the text boxes are.
    pub fn color_picker(
        &mut self,
        slot: ColorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPickerState> {
        if let Some(entity) = self.color_pickers.get(&slot) {
            return entity.clone();
        }
        // "No colour" has no sliders of its own: the picker starts from black,
        // which is only what the wheel shows until the user picks something.
        let color = theme::hsla_of(slot.color(self).unwrap_or([0, 0, 0]));
        let entity = cx.new(|cx| ColorPickerState::new(window, cx).default_value(color));
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |this, _picker, event: &ColorPickerEvent, window, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    this.color_picked(slot, *color, window, cx);
                }
            },
        );
        self._input_subscriptions.push(subscription);
        self.color_pickers.insert(slot, entity.clone());
        entity
    }

    /// Folds a colour chosen in the picker back into the state the exporter
    /// reads, the hex box beside the swatch, and the saved config.
    fn color_picked(
        &mut self,
        slot: ColorSlot,
        color: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rgb = theme::rgb_of(color);
        match slot {
            ColorSlot::Stroke => {
                self.schematic_stroke_color = rgb;
                self.schematic_stroke_hex = color_to_hex(rgb);
            }
            ColorSlot::Fill => {
                self.schematic_fill_color = Some(rgb);
                self.schematic_fill_hex = color_to_hex(rgb);
            }
        }
        let hex = color_to_hex(rgb);
        let input = self.input(slot.input_slot(), window, cx);
        input.update(cx, |state, cx| state.set_value(hex, window, cx));
        self.color_sync[slot.index()] = false;
        self.mark_config_dirty();
        cx.notify();
    }

    /// Picks the "no colour" candidate: the fill stops painting the symbol body
    /// and the popup closes, the way a palette choice does.
    pub fn clear_color(&mut self, slot: ColorSlot, window: &mut Window, cx: &mut Context<Self>) {
        if slot == ColorSlot::Fill {
            self.schematic_fill_color = None;
            self.schematic_fill_hex = String::new();
        }
        let input = self.input(slot.input_slot(), window, cx);
        input.update(cx, |state, cx| state.set_value("", window, cx));
        if let Some(picker) = self.color_pickers.get(&slot).cloned() {
            picker.update(cx, |state, cx| {
                state.clear_value(window, cx);
                state.set_open(false, cx);
            });
        }
        self.color_sync[slot.index()] = false;
        self.mark_config_dirty();
        cx.notify();
    }

    /// Pushes a colour typed into a hex box into its picker. The slider drag is
    /// the picker's own business, so this only runs for boxes the user edited.
    pub fn sync_color_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for slot in ColorSlot::ALL {
            if !self.color_sync[slot.index()] {
                continue;
            }
            let Some(picker) = self.color_pickers.get(&slot).cloned() else {
                continue;
            };
            let color = slot.color(self).map(theme::hsla_of);
            picker.update(cx, |state, cx| match color {
                Some(color) => state.set_value(color, window, cx),
                None => state.clear_value(window, cx),
            });
            self.color_sync[slot.index()] = false;
        }
    }

    /// Drops both picker popups, which the target switch does when it hides the
    /// colour rows out from under an open popup.
    pub fn close_color_pickers(&mut self, cx: &mut Context<Self>) {
        for picker in self.color_pickers.values() {
            picker.update(cx, |state, cx| state.set_open(false, cx));
        }
    }

    /// The colour a swatch shows, or `None` for the fill's "no colour".
    pub fn color_of(&self, slot: ColorSlot) -> Option<[u8; 3]> {
        slot.color(self)
    }

    fn input_seed(&self, slot: InputSlot) -> (&'static str, String) {
        let state = self.state.lock().ok();
        let field = |get: fn(&MonitorState) -> String| {
            state.as_ref().map(|state| get(state)).unwrap_or_default()
        };
        match slot {
            InputSlot::MatchedPath => (
                locale::t("BOM CSV file", "BOM CSV 文件"),
                field(|state| state.matched_save_path.clone()),
            ),
            InputSlot::ExportPath => (
                locale::t("Export directory", "导出目录"),
                field(|state| state.export_output_path.clone()),
            ),
            InputSlot::LibraryName => (
                locale::t("Library name", "库名称"),
                field(|state| state.export_library_name.clone()),
            ),
            InputSlot::Parallel => (
                locale::t("Parallel jobs", "并行任务数"),
                field(|state| state.export_parallel.to_string()),
            ),
            InputSlot::StrokeHex => ("#RRGGBB", self.schematic_stroke_hex.clone()),
            InputSlot::FillHex => (
                locale::t("no colour", "无颜色"),
                self.schematic_fill_hex.clone(),
            ),
            InputSlot::FontSearch => (locale::t("Search fonts", "搜索字体"), String::new()),
        }
    }

    /// Folds an edited box back into the state it stands for.
    fn input_changed(&mut self, slot: InputSlot, cx: &mut Context<Self>) {
        let value = self.input_value(slot, cx);
        match slot {
            InputSlot::MatchedPath => {
                if let Ok(mut state) = self.state.lock() {
                    state.set_matched_save_path(crate::monitor::normalize_path_string(&value));
                }
            }
            InputSlot::ExportPath => {
                if let Ok(mut state) = self.state.lock() {
                    state.export_output_path = crate::monitor::normalize_path_string(&value);
                }
            }
            InputSlot::LibraryName => {
                if let Ok(mut state) = self.state.lock() {
                    state.export_library_name = value;
                }
            }
            InputSlot::Parallel => {
                if let Ok(parallel) = value.trim().parse::<usize>() {
                    if let Ok(mut state) = self.state.lock() {
                        state.set_export_parallel(parallel.clamp(1, 64));
                    }
                }
            }
            InputSlot::StrokeHex => {
                if let Some(color) = parse_hex_color(&value) {
                    self.schematic_stroke_color = color;
                    self.schematic_stroke_hex = color_to_hex(color);
                    self.color_sync[ColorSlot::Stroke.index()] = true;
                }
            }
            InputSlot::FillHex => {
                if let Some(color) = parse_hex_color(&value) {
                    self.schematic_fill_color = Some(color);
                    self.schematic_fill_hex = color_to_hex(color);
                    self.color_sync[ColorSlot::Fill.index()] = true;
                } else if value.trim().is_empty() {
                    // An emptied box is the "no colour" choice written out.
                    self.schematic_fill_color = None;
                    self.schematic_fill_hex = String::new();
                    self.color_sync[ColorSlot::Fill.index()] = true;
                }
            }
            InputSlot::FontSearch => {
                cx.notify();
                return;
            }
        }
        self.mark_config_dirty();
        cx.notify();
    }

    /// The repaint work the egui `update` did on a timer: follow the system
    /// theme, collect file-dialog answers, flush config writes, expire notices.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.poll_file_dialog();
        self.process_config_save();
        self.sync_system_theme(cx);
        self.check_for_updates_when_due();
        if let Some(show_until) = self.toast_state.show_until {
            if Instant::now() > show_until {
                self.toast_state.message = None;
                self.toast_state.show_until = None;
            }
        }
    }

    /// Asks GitHub once the interval has passed. Nobody has to go looking for a
    /// release: the answer is announced in the status bar.
    fn check_for_updates_when_due(&mut self) {
        if Instant::now() < self.next_update_check {
            return;
        }
        self.next_update_check = Instant::now() + UPDATE_CHECK_INTERVAL;
        self.run_update_check(false);
    }

    fn sync_system_theme(&mut self, cx: &mut Context<Self>) {
        if self.last_theme_check.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_theme_check = Instant::now();
        let dark_mode = theme::detect_dark_mode();
        if dark_mode != theme::is_dark_mode() {
            theme::set_dark_mode(dark_mode);
            KitTheme::change(
                if dark_mode {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                None,
                cx,
            );
        }
    }

    /// Asks GitHub for the latest release on a worker thread, exactly as the
    /// egui page did, and parks the answer where the Updates panel reads it.
    /// This is the button on the More page, so a failure is worth reporting.
    pub fn start_update_check(&self) {
        self.run_update_check(true);
    }

    /// The check itself. A background one keeps quiet when it cannot reach
    /// GitHub: an error the user never asked for, on a page they never opened,
    /// is worse than no answer at all.
    fn run_update_check(&self, report_errors: bool) {
        if let Ok(mut state) = self.update_state.lock() {
            if state.checking {
                return;
            }
            // The last answer stays on screen while the new one is on its way,
            // so a reminder does not blink out every time the check re-runs.
            state.checking = true;
            state.error = None;
        }

        let state = Arc::clone(&self.update_state);
        std::thread::spawn(move || {
            let result = (|| -> Result<GitHubRelease, String> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async { fetch_latest_release().await })
            })();

            if let Ok(mut state) = state.lock() {
                state.checking = false;
                state.checked = true;
                match result {
                    Ok(release) => {
                        state.latest_version =
                            Some(release.tag_name.trim_start_matches('v').to_string());
                        state.release_url = Some(release.html_url);
                    }
                    Err(error) => {
                        if report_errors {
                            state.error = Some(format!("Update check failed: {error}"));
                        }
                    }
                }
            }
        });
    }

    /// The newer release to tell the user about, with the page that offers it.
    /// `None` while the check is in flight, when the build is current, and when
    /// the check could not reach GitHub.
    pub fn available_update(&self) -> Option<(String, String)> {
        let state = self.update_state.lock().ok()?;
        let latest = state.latest_version.clone()?;
        if !version_is_newer(&latest, env!("CARGO_PKG_VERSION")) {
            return None;
        }
        let url = state
            .release_url
            .clone()
            .unwrap_or_else(|| RELEASES_WEB_URL.to_string());
        Some((latest, url))
    }

    pub fn start_file_dialog(&mut self, kind: FileDialogKind) {
        if self.file_dialog_running {
            return;
        }
        self.file_dialog_running = true;
        self.file_dialog_start_time = Some(Instant::now());
        let sender = self.file_dialog_tx.clone();
        std::thread::spawn(move || {
            let path = match kind {
                FileDialogKind::SaveCsv => rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_file_name("matched.csv")
                    .save_file(),
                FileDialogKind::PickFolder => rfd::FileDialog::new().pick_folder(),
            };
            let _ = sender.send(FileDialogResult { kind, path });
        });
    }

    fn poll_file_dialog(&mut self) {
        if let Some(start_time) = self.file_dialog_start_time {
            if start_time.elapsed() > Duration::from_secs(30) {
                self.file_dialog_running = false;
                self.file_dialog_start_time = None;
                self.show_toast_error(locale::t("File dialog timeout", "文件对话框超时"));
            }
        }

        while let Ok(result) = self.file_dialog_rx.try_recv() {
            self.file_dialog_running = false;
            self.file_dialog_start_time = None;
            if let Some(path) = result.path {
                if let Ok(mut state) = self.state.lock() {
                    let path = crate::monitor::normalize_path_string(&path.to_string_lossy());
                    match result.kind {
                        FileDialogKind::SaveCsv => state.set_matched_save_path(path),
                        FileDialogKind::PickFolder => state.set_export_output_path(path),
                    }
                }
                self.mark_config_dirty();
            }
        }
    }

    pub fn show_toast_success(&mut self, message: impl Into<String>) {
        self.toast_state.message = Some(ToastMessage::Success(message.into()));
        self.toast_state.show_until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub fn show_toast_error(&mut self, message: impl Into<String>) {
        self.toast_state.message = Some(ToastMessage::Error(message.into()));
        self.toast_state.show_until = Some(Instant::now() + Duration::from_secs(4));
    }

    pub fn show_toast_info(&mut self, message: impl Into<String>) {
        self.toast_state.message = Some(ToastMessage::Info(message.into()));
        self.toast_state.show_until = Some(Instant::now() + Duration::from_secs(2));
    }

    pub fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
        self.config_save_time = Some(Instant::now() + Duration::from_millis(500));
    }

    fn process_config_save(&mut self) {
        if !self.config_dirty {
            return;
        }
        if let Some(save_time) = self.config_save_time {
            if Instant::now() >= save_time {
                self.save_config();
                self.config_dirty = false;
                self.config_save_time = None;
            }
        }
    }

    pub fn save_config(&self) {
        if let Ok(state) = self.state.lock() {
            snapshot_config(
                &state,
                locale::lang(),
                self.show_donate,
                self.theme_preset.clone(),
                self.schematic_stroke_color,
                self.schematic_fill_color,
                self.font_latin_choice.clone(),
                self.font_han_choice.clone(),
            )
            .save();
        }
    }

    /// Re-labels every text box whose hint text depends on the language. The
    /// placeholders are baked in when a box is created, so switching language
    /// has to push new ones, or a box keeps the language it was born in.
    pub fn refresh_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_input.update(cx, |state, cx| {
            state.set_placeholder(
                locale::t("Example: C2040, C42", "示例：C2040, C42"),
                window,
                cx,
            );
        });
        for (slot, en, zh) in [
            (InputSlot::MatchedPath, "BOM CSV file", "BOM CSV 文件"),
            (InputSlot::ExportPath, "Export directory", "导出目录"),
            (InputSlot::LibraryName, "Library name", "库名称"),
            (InputSlot::Parallel, "Parallel jobs", "并行任务数"),
            (InputSlot::FontSearch, "Search fonts", "搜索字体"),
        ] {
            if let Some(entity) = self.inputs.get(&slot).cloned() {
                entity.update(cx, |state, cx| {
                    state.set_placeholder(locale::t(en, zh), window, cx);
                });
            }
        }
    }

    /// Applies the Roman-script face the Font page picked and remembers it.
    pub fn set_latin_font(&mut self, family: String, cx: &mut Context<Self>) {
        self.font_latin = family.clone().into();
        self.font_latin_choice = Some(family.clone());
        self.latin_choices = fonts::choices(
            &self.font_names,
            fonts::FontSlot::Latin,
            self.font_latin_choice.as_deref(),
        );
        // Keep the kit's own widgets (text boxes, tooltips) on the same face.
        cx.global_mut::<KitTheme>().font_family = family.clone().into();
        fonts::remember(fonts::ui_font(family.into(), self.han_fallbacks.clone()));
        self.mark_config_dirty();
    }

    /// Applies the Han face and rebuilds the fallback chain behind it.
    pub fn set_han_font(&mut self, family: String) {
        self.han_fallbacks = FontFallbacks::from_fonts(fonts::han_chain(Some(&family)));
        self.font_han_choice = Some(family);
        self.han_choices = fonts::choices(
            &self.font_names,
            fonts::FontSlot::Han,
            self.font_han_choice.as_deref(),
        );
        fonts::remember(fonts::ui_font(
            self.font_latin.clone(),
            self.han_fallbacks.clone(),
        ));
        self.mark_config_dirty();
    }

    pub fn set_keyword_from_modes(&mut self) {
        let keyword = build_keyword(self.match_quick, self.match_full);
        if let Ok(mut state) = self.state.lock() {
            state.set_keyword(keyword);
        }
    }

    fn top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .w_full()
            .h(px(64.0))
            .px(px(14.0))
            .py(px(5.0))
            .bg(theme::surface())
            .border_b(px(1.0))
            .border_color(theme::border())
            .child(self.nav_item(Page::Monitor, locale::t("Monitor", "监听"), cx))
            .child(self.nav_item(Page::History, locale::t("History", "历史"), cx))
            .child(self.nav_item(Page::Export, locale::t("Export", "导出"), cx))
            .child(self.nav_item(Page::Font, locale::t("Font", "字体"), cx))
            .child(self.nav_item(Page::More, locale::t("More", "更多"), cx))
            .child(div().flex_1())
            .child(self.pin_toggle(cx))
            .child(self.monitoring_label())
    }

    fn nav_item(&mut self, page: Page, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        components::top_bar_button(
            format!("nav-{page:?}"),
            116.0,
            label,
            self.page == page,
            cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.page = page;
                cx.notify();
            }),
        )
        .into_any_element()
    }

    fn pin_toggle(&mut self, cx: &mut Context<Self>) -> AnyElement {
        components::hint(
            components::top_bar_button(
                "pin-toggle",
                82.0,
                locale::t("PIN", "置顶"),
                self.pin_on_top,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.pin_on_top = !this.pin_on_top;
                    set_always_on_top(this.pin_on_top);
                    cx.notify();
                }),
            ),
            locale::t("Keep window on top", "窗口置顶"),
            HintArea::StatusBar,
            cx,
        )
        .into_any_element()
    }

    fn monitoring_label(&self) -> AnyElement {
        let monitoring = self
            .state
            .lock()
            .map(|state| state.monitoring)
            .unwrap_or(false);
        let (label, color) = if monitoring {
            (locale::t("● Listening", "● 监听中"), theme::success())
        } else {
            (locale::t("● Paused", "● 已暂停"), theme::danger())
        };
        div()
            .text_size(px(theme::scaled_font(24.0)))
            .text_color(color)
            .child(label)
            .into_any_element()
    }

    fn central(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Monitor => self.monitor_page(window, cx),
            Page::History => self.history_page(cx),
            Page::Export => self.export_page(window, cx),
            Page::More => self.more_page(cx),
            Page::Font => self.font_page(window, cx),
        };

        // A flex column, not the block default: a page that fills the space it
        // is given does it with `flex_1`, and in a block container that does
        // nothing at all — the page then grows to its content and pushes its
        // bottom, and the status bar, past the window.
        div()
            .id("central-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .bg(theme::app_bg())
            .px(px(24.0))
            .py(px(18.0))
            .child(page)
    }

    fn status_bar(&self, window: &Window) -> impl IntoElement {
        let counts = self
            .state
            .lock()
            .map(|state| {
                format!(
                    "{} {} | {} {}",
                    locale::t("History:", "历史:"),
                    state.history.len(),
                    locale::t("Matched:", "匹配:"),
                    state.matched.len()
                )
            })
            .unwrap_or_default();

        // The two ends are sized to their own text, so neither can be clipped,
        // and they are kept the same width so the middle column still lands
        // exactly on the window centre. The middle then takes all the room that
        // is left, which is more than an equal third gave it.
        //
        // Sizing by a fixed third was the bug: a wider font or a narrower window
        // pushed the copyright past its column, and right-aligning then clipped
        // the start of the text — "yright (c) Ref42...".
        const PADDING: f32 = 14.0;
        const GAP: f32 = 16.0;
        // What the middle column keeps when the two ends ask for everything: the
        // notice is the point of the bar, so it never collapses to nothing.
        const MIN_MIDDLE: f32 = 160.0;
        const COPYRIGHT: &str = "Copyright (c) Ref42. All rights reserved.";
        let font_size = px(theme::scaled_font(24.0));
        let font = fonts::ui_font(self.font_latin.clone(), self.han_fallbacks.clone());
        let text_width = |text: &str| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: theme::text_color().into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(
                    SharedString::from(text.to_string()),
                    font_size,
                    std::slice::from_ref(&run),
                    None,
                )
                .width()
                .as_f32()
        };
        // The ends share what the middle can spare, so a wide font or a narrow
        // window cannot collapse the notice away.
        let available =
            (window.viewport_size().width.as_f32() - PADDING * 2.0 - GAP * 2.0).max(0.0);
        let side = text_width(&counts)
            .max(text_width(COPYRIGHT))
            .ceil()
            .min(((available - MIN_MIDDLE) / 2.0).max(64.0))
            + 2.0;

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(GAP))
            .w_full()
            .h(px(52.0))
            .px(px(PADDING))
            .py(px(5.0))
            .bg(theme::surface())
            .border_t(px(1.0))
            .border_color(theme::border())
            .text_size(px(theme::scaled_font(24.0)))
            .text_color(theme::text_color())
            .child(
                div()
                    .w(px(side))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(counts),
            )
            .child(
                // Equal ends put this column on the window centre, so a notice
                // always lands in the middle of the bar.
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(self.status_message()),
            )
            .child(
                div()
                    .w(px(side))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_align(TextAlign::Right)
                    .text_color(theme::muted())
                    .child(COPYRIGHT),
            )
    }

    /// Whatever the status bar is saying right now: a running notice if there is
    /// one, otherwise the hint for the control under the pointer, otherwise the
    /// waiting update. The first two use the same filled box; the update is the
    /// same box again, but it takes a click.
    fn status_message(&self) -> AnyElement {
        let (text, accent) = match self.toast_state.message {
            Some(ToastMessage::Success(ref message)) => (message.clone(), theme::success()),
            Some(ToastMessage::Error(ref message)) => (message.clone(), theme::danger()),
            Some(ToastMessage::Info(ref message)) => (message.clone(), theme::primary()),
            None => match self.hover_hint {
                Some((HintArea::StatusBar, ref hint)) => (hint.to_string(), theme::primary()),
                _ => {
                    let Some((version, url)) = self.available_update() else {
                        return div().into_any_element();
                    };
                    return components::status_button(
                        "update-available",
                        format!(
                            "{} v{version} — {}",
                            locale::t("Update to", "更新到"),
                            locale::t("download", "下载")
                        ),
                        theme::primary(),
                        move |_event: &ClickEvent, _window, cx| cx.open_url(&url),
                    )
                    .into_any_element();
                }
            },
        };
        components::status_box(text, accent).into_any_element()
    }
}

impl Render for SeexUi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .font(fonts::ui_font(
                self.font_latin.clone(),
                self.han_fallbacks.clone(),
            ))
            .bg(theme::app_bg())
            .child(self.top_bar(cx))
            .child(self.central(window, cx))
            .child(self.status_bar(window))
    }
}

pub fn apply_config_to_state(config: &AppConfig, state: &mut MonitorState) {
    state.set_export_output_path(config.export.output_path.clone());
    state.set_export_target(config.export.target.clone());
    state.set_export_mode(config.export.mode.clone());
    state.set_export_kicad_mode(config.export.kicad_mode.clone());
    state.set_export_cadence_mode(config.export.cadence_mode.clone());
    state.set_export_merge(config.export.merge);
    state.set_export_append(config.export.append);
    state.set_export_library_name(config.export.library_name.clone());
    state.set_export_parallel(config.export.parallel);
    state.set_export_continue_on_error(config.export.continue_on_error);
    state.set_export_lcsc_english(config.export.lcsc_english);
    state.set_export_force(config.export.force);
    state.set_export_remove_model_logo(config.export.remove_model_logo);
    state.set_matched_save_path(config.monitor.matched_save_path.clone());
    state.set_bom_language(config.monitor.bom_language.clone());
}

pub fn snapshot_config(
    state: &MonitorState,
    language: Lang,
    show_donate: bool,
    theme: String,
    schematic_stroke_color: [u8; 3],
    schematic_fill_color: Option<[u8; 3]>,
    latin_font: Option<String>,
    han_font: Option<String>,
) -> AppConfig {
    AppConfig {
        export: ExportConfig {
            target: state.export_target.clone(),
            output_path: state.export_output_path.clone(),
            mode: state.export_mode.clone(),
            kicad_mode: state.export_kicad_mode.clone(),
            cadence_mode: state.export_cadence_mode.clone(),
            merge: state.export_merge,
            append: state.export_append,
            library_name: state.export_library_name.clone(),
            parallel: state.export_parallel,
            continue_on_error: state.export_continue_on_error,
            lcsc_english: state.export_lcsc_english,
            force: state.export_force,
            schematic_stroke_color,
            schematic_fill_color,
            remove_model_logo: state.export_remove_model_logo,
        },
        monitor: MonitorConfig {
            matched_save_path: state.matched_save_path.clone(),
            bom_language: state.bom_language.clone(),
        },
        ui_language: match language {
            Lang::En => "english".to_string(),
            Lang::Zh => "chinese".to_string(),
        },
        show_donate,
        theme,
        latin_font,
        han_font,
    }
}

pub fn build_keyword(match_quick: bool, match_full: bool) -> String {
    let mut parts = Vec::new();
    if match_full {
        parts.push(PATTERN_FULL);
    }
    if match_quick {
        parts.push(PATTERN_QUICK);
    }
    parts.join("||")
}

pub fn color_to_hex(color: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if value.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ])
}

pub fn version_is_newer(latest: &str, current: &str) -> bool {
    let parse = |version: &str| {
        version
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let latest = parse(latest);
    let current = parse(current);
    for index in 0..latest.len().max(current.len()) {
        let left = latest.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

pub async fn fetch_latest_release() -> Result<GitHubRelease, String> {
    let client = reqwest::Client::new();
    let api_response = client
        .get(RELEASES_API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "SeEx")
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if api_response.status().is_success() {
        return api_response
            .json::<GitHubRelease>()
            .await
            .map_err(|error| error.to_string());
    }

    let api_status = api_response.status();
    let web_response = client
        .get(RELEASES_WEB_URL)
        .header("User-Agent", "SeEx")
        .send()
        .await
        .map_err(|error| {
            format!("GitHub API returned {api_status}; web fallback failed: {error}")
        })?;
    let web_url = web_response.url().to_string();
    let tag_name = web_url
        .split("/releases/tag/")
        .nth(1)
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.trim_end_matches('/').to_string())
        .ok_or_else(|| {
            format!("GitHub API returned {api_status}; latest release tag was not found")
        })?;

    Ok(GitHubRelease {
        tag_name,
        html_url: web_url,
    })
}

pub fn copy_to_clipboard(text: &str) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_string());
    }
}

/// Decodes the embedded donation code, keeping the aspect ratio the More page
/// needs to size it.
fn load_donation(bytes: Vec<u8>, format: ImageFormat) -> (Arc<Image>, f32) {
    let aspect = image::load_from_memory(&bytes)
        .map(|image| image.width() as f32 / image.height().max(1) as f32)
        .unwrap_or(1.0);
    (Arc::new(Image::from_bytes(format, bytes)), aspect)
}

pub fn first_line(content: &str, max_chars: usize) -> String {
    let mut line = content.lines().next().unwrap_or_default().to_string();
    if line.chars().count() > max_chars {
        line = line.chars().take(max_chars).collect::<String>() + "…";
    }
    line
}

#[cfg(windows)]
fn set_windows_app_id() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let app_id: Vec<u16> = OsStr::new("Ref42.SeEx")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ =
            windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }
}

#[cfg(not(windows))]
fn set_windows_app_id() {}
