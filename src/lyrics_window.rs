// Jendela lirik (tombol Aa), port LyricsWindow NotchMusic: kaca acrylic berwarna tema notch,
// tombol bulat, dropdown bertema, bagian "Lainnya" yang bisa dibuka, dan teks lirik lengkap.
// Semua digambar sendiri dengan Direct2D dalam DIP, mengikuti ukuran dan warna XAML aslinya.

use std::cell::RefCell;

use windows::core::{w, Result, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
    D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_CUSTOM,
    D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_LINE_JOIN_MITER, D2D1_LINE_JOIN_ROUND, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_SPACING_METHOD_UNIFORM,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetStockObject, InvalidateRect, ScreenToClient, BLACK_BRUSH, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_CONTROL,
    VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::draw::Theme;

// Ukuran dari LyricsWindow asli (DIP).
const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 620.0;
const MIN_WIDTH: f32 = 380.0;
const MIN_HEIGHT: f32 = 420.0;
const MARGIN: f32 = 18.0;
const CONTROL_H: f32 = 28.0;
const WRAP_LINE: f32 = 34.0; // Tombol 28 + margin bawah 6 di WrapPanel.
const BAR_W: f32 = 16.0; // ClipboardScrollBar: lebar 12 + margin kiri 4.
const ITEM_H: f32 = 26.0; // Item dropdown: padding 5 + teks.
const SCROLL_LINE: f32 = 16.0;
const RESULTS_MAX_H: f32 = 220.0;
const MAX_QUERY: usize = 512;
const MODES: [&str; 3] = ["Otomatis", "Lirik asli", "Romaji (Jepang)"];

const TIMER_CARET: usize = 1;
const TIMER_REPEAT: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;

// Warna bawaan template WPF (Aero2) untuk TextBox dan Expander.
const TEXTBOX_HOVER: u32 = 0x7EB4EA;
const TEXTBOX_FOCUS: u32 = 0x569DE5;
const SELECTION: u32 = 0x0078D7;
const SCROLL_TRACK: (u32, f32) = (0x8E9162, 0x26 as f32 / 255.0);

pub enum Action {
    Search,
    Choose(usize),
    Mode(u32),
    Earlier,
    Later,
    ResetOffset,
    Save,
    Load,
    Saved,
    Closed,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    None,
    Mode,
    Search,
    Results,
    Earlier,
    Later,
    Reset,
    More,
    Title,
    Artist,
    Save,
    Load,
    Saved,
    Up,
    Down,
    Thumb,
    PageUp,
    PageDown,
    Item(usize),
}

struct Popup {
    combo: Part,
    highlight: Option<usize>,
    scroll: f32,
}

struct Label {
    x: f32,
    y: f32,
    layout: IDWriteTextLayout,
    alpha: f32,
}

/// Kotak teks satu baris (judul/artis) dalam UTF-16, seperti TextBox WPF.
#[derive(Default)]
struct Edit {
    text: Vec<u16>,
    caret: usize,
    anchor: usize,
    scroll: f32,
}

struct Bar {
    up: D2D_RECT_F,
    down: D2D_RECT_F,
    track: D2D_RECT_F,
    thumb: D2D_RECT_F,
}

struct Formats {
    normal: IDWriteTextFormat,
    semibold: IDWriteTextFormat,
    button: IDWriteTextFormat,
    status: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    body: IDWriteTextFormat,
}

struct Ui {
    hwnd: HWND,
    dpi: f32,
    acrylic: bool,
    theme: [u32; 4], // background, surface, soft, accent
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    round: ID2D1StrokeStyle,
    dashed: ID2D1StrokeStyle,
    formats: Formats,
    line: f32, // Tinggi satu baris Segoe UI 12.

    heading: String,
    status: String,
    offset: String,
    body: String,
    mode: usize,
    choices: Vec<String>,
    chosen: Option<usize>,
    can_save: bool,
    synced: bool,
    can_load: bool,
    title: Edit,
    artist: Edit,
    expanded: bool,

    stale: bool,
    boxes: Vec<(Part, D2D_RECT_F)>,
    labels: Vec<Label>,
    content_h: f32,
    bar: bool,
    scroll: f32,

    hover: Part,
    pressed: Part,
    focus: Part,
    focus_visible: bool,
    caret_on: bool,
    tracking: bool,
    popup: Option<Popup>,
    drag: Option<(f32, f32)>,
    selecting: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<fn(Action)>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|cell| cell.try_borrow_mut().ok().and_then(|mut ui| ui.as_mut().map(f)))
}

fn dispatch(action: Option<Action>) {
    if let (Some(action), Some(handler)) = (action, HANDLER.with(|h| *h.borrow())) {
        handler(action);
    }
}

fn rgba(rgb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a,
    }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn shifted(r: D2D_RECT_F, dy: f32) -> D2D_RECT_F {
    rect(r.left, r.top + dy, r.right, r.bottom + dy)
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

fn translation(y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: y }
}

fn is_high(unit: u16) -> bool {
    (0xD800..0xDC00).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

impl Edit {
    fn set(&mut self, text: &str) {
        self.text = text.encode_utf16().take(MAX_QUERY).collect();
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.scroll = 0.0;
    }

    fn string(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }

    fn range(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn selected(&self) -> String {
        let (a, b) = self.range();
        String::from_utf16_lossy(&self.text[a..b])
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.range();
        if a == b {
            return false;
        }
        self.text.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    fn insert(&mut self, units: &[u16]) {
        self.delete_selection();
        let room = MAX_QUERY.saturating_sub(self.text.len());
        let units = &units[..units.len().min(room)];
        self.text.splice(self.caret..self.caret, units.iter().copied());
        self.caret += units.len();
        self.anchor = self.caret;
    }

    fn prev(&self, i: usize) -> usize {
        if i == 0 {
            return 0;
        }
        let mut j = i - 1;
        if j > 0 && is_low(self.text[j]) && is_high(self.text[j - 1]) {
            j -= 1;
        }
        j
    }

    fn next(&self, i: usize) -> usize {
        let n = self.text.len();
        if i >= n {
            return n;
        }
        let mut j = i + 1;
        if j < n && is_high(self.text[i]) && is_low(self.text[j]) {
            j += 1;
        }
        j
    }

    fn word_left(&self, i: usize) -> usize {
        let mut j = i;
        while j > 0 && self.text[j - 1] == b' ' as u16 {
            j -= 1;
        }
        while j > 0 && self.text[j - 1] != b' ' as u16 {
            j -= 1;
        }
        j
    }

    fn word_right(&self, i: usize) -> usize {
        let (mut j, n) = (i, self.text.len());
        while j < n && self.text[j] != b' ' as u16 {
            j += 1;
        }
        while j < n && self.text[j] == b' ' as u16 {
            j += 1;
        }
        j
    }
}

pub struct LyricsWindow {
    pub hwnd: HWND,
}

unsafe fn text_format(dwrite: &IDWriteFactory, size: f32, weight: DWRITE_FONT_WEIGHT, center: bool, wrap: bool) -> Result<IDWriteTextFormat> {
    let format =
        dwrite.CreateTextFormat(w!("Segoe UI"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("id-ID"))?;
    if center {
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    }
    format.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
    Ok(format)
}

unsafe fn stroke(factory: &ID2D1Factory, dashed: bool) -> Result<ID2D1StrokeStyle> {
    let cap = if dashed { D2D1_CAP_STYLE_FLAT } else { D2D1_CAP_STYLE_ROUND };
    factory.CreateStrokeStyle(
        &D2D1_STROKE_STYLE_PROPERTIES {
            startCap: cap,
            endCap: cap,
            dashCap: cap,
            lineJoin: if dashed { D2D1_LINE_JOIN_MITER } else { D2D1_LINE_JOIN_ROUND },
            miterLimit: 10.0,
            dashStyle: if dashed { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
            dashOffset: 0.0,
        },
        if dashed { Some(&[1.0f32, 2.0][..]) } else { None },
    )
}

fn theme_colors(theme: &Theme) -> [u32; 4] {
    [theme.background, theme.surface, theme.soft, theme.accent]
}

impl LyricsWindow {
    /// Buka jendela dengan warna tema notch. `handler` menerima aksi dari tombol dan daftar.
    pub fn open(instance: HINSTANCE, theme: &Theme, topmost: bool, dpi: f32, handler: fn(Action)) -> Result<LyricsWindow> {
        unsafe {
            HANDLER.with(|h| *h.borrow_mut() = Some(handler));
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: LoadIconW(instance, PCWSTR(1 as _)).unwrap_or_default(),
                // Hitam = tembus ke kaca sebelum gambar pertama.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: w!("NotchRustLyrics"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let scale = dpi / 96.0;
            let (width, height) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
            let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
            let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
            let ex = if topmost { WS_EX_TOPMOST } else { WINDOW_EX_STYLE(0) };
            let hwnd = CreateWindowExW(
                ex,
                w!("NotchRustLyrics"),
                w!("Lirik · NotchRust"),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                width,
                height,
                None,
                None,
                instance,
                None,
            )?;
            // Title bar gelap, lalu kaca acrylic (DWMSBT_TRANSIENTWINDOW) seperti versi WPF.
            // Windows lama yang tidak mendukungnya tetap memakai latar solid.
            let on: i32 = 1;
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &on as *const i32 as *const _, 4);
            let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let backdrop: i32 = 3;
            let acrylic = DwmExtendFrameIntoClientArea(hwnd, &margins).is_ok()
                && DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(38), &backdrop as *const i32 as *const _, 4).is_ok();

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let body = text_format(&dwrite, 15.0, DWRITE_FONT_WEIGHT_NORMAL, false, true)?;
            body.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, 24.0, 19.0)?;
            let formats = Formats {
                normal: text_format(&dwrite, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false, false)?,
                semibold: text_format(&dwrite, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false, true)?,
                button: text_format(&dwrite, 12.0, DWRITE_FONT_WEIGHT_NORMAL, true, false)?,
                status: text_format(&dwrite, 11.0, DWRITE_FONT_WEIGHT_NORMAL, false, true)?,
                heading: text_format(&dwrite, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, false, true)?,
                body,
            };
            let mut ui = Ui {
                hwnd,
                dpi: GetDpiForWindow(hwnd).max(96) as f32,
                acrylic,
                theme: theme_colors(theme),
                round: stroke(&factory, false)?,
                dashed: stroke(&factory, true)?,
                factory,
                dwrite,
                target: None,
                formats,
                line: 16.0,
                heading: String::new(),
                status: String::new(),
                offset: String::new(),
                body: String::new(),
                mode: 0,
                choices: Vec::new(),
                chosen: None,
                can_save: false,
                synced: true,
                can_load: false,
                title: Edit::default(),
                artist: Edit::default(),
                expanded: false,
                stale: true,
                boxes: Vec::new(),
                labels: Vec::new(),
                content_h: 0.0,
                bar: false,
                scroll: 0.0,
                hover: Part::None,
                pressed: Part::None,
                focus: Part::None,
                focus_visible: false,
                caret_on: false,
                tracking: false,
                popup: None,
                drag: None,
                selecting: false,
            };
            ui.line = ui.measure("Ag", &ui.formats.normal).1;
            UI.with(|cell| *cell.borrow_mut() = Some(ui));
            let _ = ShowWindow(hwnd, SW_SHOW);
            Ok(LyricsWindow { hwnd })
        }
    }

    fn update(&self, f: impl FnOnce(&mut Ui) -> bool) {
        with_ui(|ui| {
            if f(ui) {
                ui.stale = true;
                ui.invalidate();
            }
        });
    }

    pub fn set_theme(&self, theme: &Theme) {
        self.update(|ui| {
            ui.theme = theme_colors(theme);
            true
        });
    }

    pub fn set_topmost(&self, topmost: bool) {
        let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn set_heading(&self, text: &str) {
        self.update(|ui| replace(&mut ui.heading, text));
    }

    pub fn set_status(&self, text: &str) {
        self.update(|ui| replace(&mut ui.status, text));
    }

    pub fn set_offset(&self, seconds: f64) {
        let sign = if seconds > 0.0 { "+" } else { "" };
        let text = format!("Waktu lirik: {sign}{:.1} dtk", seconds).replace('.', ",");
        self.update(|ui| replace(&mut ui.offset, &text));
    }

    pub fn set_body(&self, text: &str) {
        self.update(|ui| replace(&mut ui.body, text));
    }

    pub fn set_query(&self, title: &str, artist: &str) {
        self.update(|ui| {
            ui.title.set(title);
            ui.artist.set(artist);
            true
        });
    }

    pub fn query(&self) -> (String, String) {
        with_ui(|ui| (ui.title.string().trim().to_string(), ui.artist.string().trim().to_string())).unwrap_or_default()
    }

    pub fn set_mode(&self, index: u32) {
        self.update(|ui| {
            let changed = ui.mode != index as usize;
            ui.mode = (index as usize).min(MODES.len() - 1);
            changed
        });
    }

    /// Isi daftar "Lirik yang dipakai"; `selected` = indeks yang sedang dipakai (bila ada).
    pub fn set_choices(&self, labels: &[String], selected: Option<usize>) {
        self.update(|ui| {
            ui.choices = labels.to_vec();
            ui.chosen = selected.filter(|&i| i < labels.len());
            if ui.popup.as_ref().is_some_and(|p| p.combo == Part::Results) {
                ui.popup = None;
            }
            true
        });
    }

    pub fn set_can_save(&self, enabled: bool, synced: bool) {
        self.update(|ui| {
            let changed = ui.can_save != enabled || ui.synced != synced;
            ui.can_save = enabled;
            ui.synced = synced;
            changed
        });
    }

    pub fn set_can_load(&self, enabled: bool) {
        self.update(|ui| {
            let changed = ui.can_load != enabled;
            ui.can_load = enabled;
            changed
        });
    }

    pub fn close(&self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn replace(slot: &mut String, text: &str) -> bool {
    if slot == text {
        return false;
    }
    *slot = text.to_string();
    true
}

impl Ui {
    fn scale(&self) -> f32 {
        self.dpi / 96.0
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    fn client(&self) -> (f32, f32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left) as f32 / self.scale(), (r.bottom - r.top) as f32 / self.scale())
    }

    // ---- Warna (UseTheme di LyricsWindow asli) ---------------------------------------------

    fn background(&self) -> D2D1_COLOR_F {
        rgba(self.theme[0], if self.acrylic { 0xB3 as f32 / 255.0 } else { 1.0 })
    }

    /// NotchSoft: isi kaca tombol, dropdown, dan kotak teks.
    fn glass(&self, opacity: f32) -> D2D1_COLOR_F {
        rgba(self.theme[2], 0x33 as f32 / 255.0 * opacity)
    }

    /// NotchSurface: tepi kaca.
    fn edge(&self, opacity: f32) -> D2D1_COLOR_F {
        rgba(self.theme[3], 0x59 as f32 / 255.0 * opacity)
    }

    /// NotchAccent: isi saat hover.
    fn lit(&self, opacity: f32) -> D2D1_COLOR_F {
        rgba(self.theme[2], 0x59 as f32 / 255.0 * opacity)
    }

    fn popup_fill(&self) -> D2D1_COLOR_F {
        rgba(self.theme[0], 0xF2 as f32 / 255.0)
    }

    // ---- Teks --------------------------------------------------------------------------------

    unsafe fn layout(&self, units: &[u16], format: &IDWriteTextFormat, width: f32) -> Result<IDWriteTextLayout> {
        self.dwrite.CreateTextLayout(units, format, width.max(1.0), 100_000.0)
    }

    unsafe fn layout_str(&self, text: &str, format: &IDWriteTextFormat, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.layout(&units, format, width)
    }

    unsafe fn measure(&self, text: &str, format: &IDWriteTextFormat) -> (f32, f32) {
        self.layout_str(text, format, 100_000.0).map(|l| size(&l)).unwrap_or((0.0, 16.0))
    }

    unsafe fn button_width(&self, text: &str) -> f32 {
        // Padding 12 kiri-kanan + tepi 1.
        self.measure(text, &self.formats.normal).0 + 26.0
    }

    unsafe fn label(&self, labels: &mut Vec<Label>, text: &str, format: &IDWriteTextFormat, x: f32, y: f32, width: f32, alpha: f32) -> f32 {
        let Ok(layout) = self.layout_str(text, format, width) else { return 0.0 };
        let height = size(&layout).1;
        labels.push(Label { x, y, layout, alpha });
        height
    }

    fn save_text(&self) -> &'static str {
        if self.synced {
            "Simpan .lrc"
        } else {
            "Simpan .txt"
        }
    }

    fn button_text(&self, part: Part) -> &'static str {
        match part {
            Part::Search => "Cari ulang",
            Part::Earlier => "Lebih awal",
            Part::Later => "Lebih lambat",
            Part::Reset => "Reset",
            Part::Save => self.save_text(),
            Part::Load => "Muat file…",
            Part::Saved => "Lirik tersimpan…",
            _ => "",
        }
    }

    fn enabled(&self, part: Part) -> bool {
        match part {
            Part::Save => self.can_save,
            Part::Load | Part::Saved => self.can_load,
            _ => true,
        }
    }

    // ---- Tata letak (StackPanel dengan margin 18 di dalam ScrollViewer) -----------------------

    fn arrange(&mut self) {
        self.stale = false;
        let (width, height) = self.client();
        self.bar = false;
        let mut content = unsafe { self.arrange_width(width) };
        if content > height {
            self.bar = true;
            content = unsafe { self.arrange_width(width - BAR_W) };
        }
        self.content_h = content;
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - self.client().1).max(0.0)
    }

    /// WrapPanel: posisi tiap item (lebar termasuk margin) dan batas bawah baris terakhir.
    fn flow(items: &[(Part, f32)], x0: f32, y: f32, width: f32) -> (Vec<(f32, f32)>, f32) {
        let (mut x, mut line) = (x0, y);
        let mut spots = Vec::new();
        for &(_, w) in items {
            if x > x0 && x + w > x0 + width + 0.01 {
                x = x0;
                line += WRAP_LINE;
            }
            spots.push((x, line));
            x += w;
        }
        (spots, line + WRAP_LINE)
    }

    unsafe fn arrange_width(&mut self, width: f32) -> f32 {
        let mut boxes = Vec::new();
        let mut labels = Vec::new();
        let f = &self.formats;
        let x0 = MARGIN;
        let cw = (width - 2.0 * MARGIN).max(40.0);
        let mut y = MARGIN;

        // Judul lagu dan status.
        y += self.label(&mut labels, &self.heading, &f.heading, x0, y, cw, 1.0);
        y += 2.0;
        y += self.label(&mut labels, &self.status, &f.status, x0, y, cw, 0.8) + 14.0;

        // Jenis lirik + cari ulang.
        let search_w = self.button_width("Cari ulang");
        boxes.push((Part::Search, rect(x0 + cw - search_w, y, x0 + cw, y + CONTROL_H)));
        boxes.push((Part::Mode, rect(x0, y, x0 + cw - search_w - 8.0, y + CONTROL_H)));
        y += CONTROL_H + 10.0;

        // Hasil yang tampil di notch.
        y += self.label(&mut labels, "Lirik yang dipakai", &f.semibold, x0, y, cw, 1.0) + 4.0;
        boxes.push((Part::Results, rect(x0, y, x0 + cw, y + CONTROL_H)));
        y += CONTROL_H + 10.0;

        // Koreksi waktu.
        let (offset_w, offset_h) = self.measure(&self.offset, &f.normal);
        let items = [
            (Part::None, offset_w.max(120.0)),
            (Part::Earlier, self.button_width("Lebih awal") + 6.0),
            (Part::Later, self.button_width("Lebih lambat") + 6.0),
            (Part::Reset, self.button_width("Reset") + 6.0),
        ];
        let (spots, bottom) = Self::flow(&items, x0, y, cw);
        for (&(part, w), &(x, line)) in items.iter().zip(&spots) {
            if part == Part::None {
                self.label(&mut labels, &self.offset, &f.normal, x, line + (WRAP_LINE - offset_h) / 2.0, 10_000.0, 1.0);
            } else {
                boxes.push((part, rect(x, line, x + w - 6.0, line + CONTROL_H)));
            }
        }
        y = bottom + 12.0;

        // Expander "Lainnya" (tepi 1 + margin tombol kepala 1, lingkaran 19).
        let header = y + 2.0;
        boxes.push((Part::More, rect(x0 + 2.0, header, x0 + cw - 2.0, header + 19.0)));
        let header_h = self.measure("Lainnya: judul/artis dan file", &f.normal).1;
        self.label(&mut labels, "Lainnya: judul/artis dan file", &f.normal, x0 + 25.0, header + (19.0 - header_h) / 2.0, cw, 1.0);
        y = header + 20.0;
        if self.expanded {
            let (cx, cw) = (x0 + 1.0, cw - 2.0);
            y += 8.0;
            y += self.label(&mut labels, "Judul dan artis untuk pencarian", &f.semibold, cx, y, cw, 1.0) + 4.0;
            let edit_h = self.line + 10.0; // Tepi 1 + padding 4, atas dan bawah.
            boxes.push((Part::Title, rect(cx, y, cx + cw, y + edit_h)));
            y += edit_h + 6.0;
            boxes.push((Part::Artist, rect(cx, y, cx + cw, y + edit_h)));
            y += edit_h + 6.0 + 4.0;
            let items = [
                (Part::Save, self.button_width(self.save_text()) + 6.0),
                (Part::Load, self.button_width("Muat file…") + 6.0),
                (Part::Saved, self.button_width("Lirik tersimpan…") + 6.0),
            ];
            let (spots, bottom) = Self::flow(&items, cx, y, cw);
            for (&(part, w), &(x, line)) in items.iter().zip(&spots) {
                boxes.push((part, rect(x, line, x + w - 6.0, line + CONTROL_H)));
            }
            y = bottom + 12.0 + 4.0;
        }
        y += 1.0 + 14.0;

        // Teks lirik lengkap.
        y += self.label(&mut labels, &self.body, &f.body, x0, y, cw, 1.0);

        self.boxes = boxes;
        self.labels = labels;
        y + MARGIN
    }

    fn rect_of(&self, part: Part) -> Option<D2D_RECT_F> {
        self.boxes.iter().find(|(p, _)| *p == part).map(|(_, r)| *r)
    }

    fn ensure_arranged(&mut self) {
        if self.stale {
            self.arrange();
        }
    }

    // ---- Scrollbar dan dropdown --------------------------------------------------------------

    /// ClipboardScrollBar: panah 12 px, jalur dengan margin 2,1, thumb dengan margin 3,0.
    fn bar_geometry(x: f32, top: f32, bottom: f32, offset: f32, viewport: f32, extent: f32) -> Bar {
        let track_len = (bottom - top - 24.0).max(0.0);
        let thumb_len = (track_len * viewport / extent.max(1.0)).clamp(12.0_f32.min(track_len), track_len);
        let max = (extent - viewport).max(1.0);
        let thumb_top = top + 12.0 + (track_len - thumb_len) * (offset / max).clamp(0.0, 1.0);
        Bar {
            up: rect(x, top, x + 12.0, top + 12.0),
            down: rect(x, bottom - 12.0, x + 12.0, bottom),
            track: rect(x, top + 12.0, x + 12.0, bottom - 12.0),
            thumb: rect(x + 3.0, thumb_top, x + 9.0, thumb_top + thumb_len),
        }
    }

    fn main_bar(&self) -> Option<Bar> {
        if !self.bar {
            return None;
        }
        let (width, height) = self.client();
        Some(Self::bar_geometry(width - 12.0, 2.0, height - 2.0, self.scroll, height, self.content_h))
    }

    fn items(&self, combo: Part) -> Vec<String> {
        match combo {
            Part::Mode => MODES.iter().map(|m| m.to_string()).collect(),
            _ => self.choices.clone(),
        }
    }

    fn selected(&self, combo: Part) -> Option<usize> {
        match combo {
            Part::Mode => Some(self.mode),
            _ => self.chosen,
        }
    }

    /// Kotak dropdown terbuka (koordinat jendela), jumlah item, dan tinggi isi.
    fn popup_rect(&self) -> Option<(D2D_RECT_F, usize, f32)> {
        let popup = self.popup.as_ref()?;
        let combo = shifted(self.rect_of(popup.combo)?, -self.scroll);
        let (width, height) = self.client();
        let items = self.items(popup.combo);
        let inner = items.len() as f32 * ITEM_H;
        let max_h = if popup.combo == Part::Results { RESULTS_MAX_H } else { height - 4.0 };
        let h = (inner + 10.0).min(max_h);
        let natural = items
            .iter()
            .map(|item| unsafe { self.measure(item, &self.formats.semibold).0 })
            .fold(0.0, f32::max)
            + 26.0
            + if inner + 10.0 > h { BAR_W } else { 0.0 };
        let combo_w = combo.right - combo.left;
        let w = natural.min(width - 4.0 - combo.left).max(combo_w);
        let mut top = combo.bottom + 4.0;
        if top + h > height - 2.0 && combo.top - 4.0 - h >= 2.0 {
            top = combo.top - 4.0 - h;
        }
        Some((rect(combo.left, top, combo.left + w, top + h), items.len(), inner))
    }

    fn popup_max_scroll(&self) -> f32 {
        self.popup_rect().map_or(0.0, |(r, _, inner)| (inner - (r.bottom - r.top - 10.0)).max(0.0))
    }

    fn open_popup(&mut self, combo: Part) {
        if self.popup.as_ref().is_some_and(|p| p.combo == combo) {
            self.popup = None;
            return;
        }
        self.popup = Some(Popup { combo, highlight: self.selected(combo), scroll: 0.0 });
        self.reveal_highlight();
    }

    fn reveal_highlight(&mut self) {
        let max = self.popup_max_scroll();
        let Some((r, _, _)) = self.popup_rect() else { return };
        let visible = r.bottom - r.top - 10.0;
        if let Some(popup) = &mut self.popup {
            if let Some(i) = popup.highlight {
                let top = i as f32 * ITEM_H;
                if top < popup.scroll {
                    popup.scroll = top;
                } else if top + ITEM_H > popup.scroll + visible {
                    popup.scroll = top + ITEM_H - visible;
                }
            }
            popup.scroll = popup.scroll.clamp(0.0, max);
        }
    }

    fn select(&mut self, combo: Part, index: usize) -> Option<Action> {
        match combo {
            Part::Mode if index != self.mode && index < MODES.len() => {
                self.mode = index;
                Some(Action::Mode(index as u32))
            }
            Part::Results if self.chosen != Some(index) && index < self.choices.len() => {
                self.chosen = Some(index);
                Some(Action::Choose(index))
            }
            _ => None,
        }
    }

    // ---- Hit test ----------------------------------------------------------------------------

    fn in_popup(&self, x: f32, y: f32) -> bool {
        self.popup_rect().is_some_and(|(r, _, _)| inside(&r, x, y))
    }

    fn hit(&self, x: f32, y: f32) -> Part {
        if let (Some(popup), Some((r, count, inner))) = (&self.popup, self.popup_rect()) {
            let scrolling = inner > r.bottom - r.top - 10.0;
            let list_right = r.right - 5.0 - if scrolling { BAR_W } else { 0.0 };
            if inside(&rect(r.left + 5.0, r.top + 5.0, list_right, r.bottom - 5.0), x, y) {
                let index = ((y - r.top - 5.0 + popup.scroll) / ITEM_H).floor();
                if index >= 0.0 && (index as usize) < count {
                    return Part::Item(index as usize);
                }
            }
            return Part::None;
        }
        if let Some(bar) = self.main_bar() {
            if inside(&bar.up, x, y) {
                return Part::Up;
            }
            if inside(&bar.down, x, y) {
                return Part::Down;
            }
            if inside(&bar.track, x, y) {
                return if y < bar.thumb.top {
                    Part::PageUp
                } else if y >= bar.thumb.bottom {
                    Part::PageDown
                } else {
                    Part::Thumb
                };
            }
        }
        let cy = y + self.scroll;
        self.boxes.iter().find(|(_, r)| inside(r, x, cy)).map_or(Part::None, |(p, _)| *p)
    }

    // ---- Kotak teks --------------------------------------------------------------------------

    fn edit(&self, part: Part) -> Option<&Edit> {
        match part {
            Part::Title => Some(&self.title),
            Part::Artist => Some(&self.artist),
            _ => None,
        }
    }

    fn edit_mut(&mut self, part: Part) -> Option<&mut Edit> {
        match part {
            Part::Title => Some(&mut self.title),
            Part::Artist => Some(&mut self.artist),
            _ => None,
        }
    }

    unsafe fn text_x(&self, part: Part, position: usize) -> f32 {
        let Some(edit) = self.edit(part) else { return 0.0 };
        let Ok(layout) = self.layout(&edit.text, &self.formats.normal, 100_000.0) else { return 0.0 };
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        let _ = layout.HitTestTextPosition(position as u32, BOOL(0), &mut x, &mut y, &mut metrics);
        x
    }

    unsafe fn position_at(&self, part: Part, x: f32) -> usize {
        let (Some(edit), Some(r)) = (self.edit(part), self.rect_of(part)) else { return 0 };
        let Ok(layout) = self.layout(&edit.text, &self.formats.normal, 100_000.0) else { return 0 };
        let (mut trailing, mut is_inside) = (BOOL(0), BOOL(0));
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        let local = x - (r.left + 9.0) + edit.scroll;
        if layout.HitTestPoint(local, 1.0, &mut trailing, &mut is_inside, &mut metrics).is_err() {
            return edit.text.len();
        }
        let position = metrics.textPosition as usize + if trailing.as_bool() { metrics.length as usize } else { 0 };
        position.min(edit.text.len())
    }

    /// Geser isi kotak teks agar kursor tetap terlihat.
    unsafe fn follow_caret(&mut self) {
        for part in [Part::Title, Part::Artist] {
            let Some(r) = self.rect_of(part) else { continue };
            let visible = (r.right - r.left - 18.0).max(1.0);
            let caret = self.edit(part).map_or(0, |e| e.caret);
            let x = self.text_x(part, caret);
            if let Some(edit) = self.edit_mut(part) {
                if x - edit.scroll > visible {
                    edit.scroll = x - visible;
                } else if x < edit.scroll {
                    edit.scroll = x;
                }
            }
        }
    }

    fn wake_caret(&mut self) {
        self.caret_on = true;
        unsafe {
            if matches!(self.focus, Part::Title | Part::Artist) {
                SetTimer(self.hwnd, TIMER_CARET, GetCaretBlinkTime().clamp(200, 2000), None);
            } else {
                let _ = KillTimer(self.hwnd, TIMER_CARET);
            }
        }
        self.invalidate();
    }

    fn set_focus(&mut self, part: Part) {
        self.focus = part;
        self.wake_caret();
    }

    unsafe fn edit_key(&mut self, key: u16, ctrl: bool, shift: bool) -> (bool, Option<Action>) {
        let part = self.focus;
        let hwnd = self.hwnd;
        if key == VK_RETURN.0 {
            return (true, Some(Action::Search));
        }
        let pasted = if ctrl && key == b'V' as u16 { crate::clipboard::paste(hwnd) } else { None };
        let Some(edit) = self.edit_mut(part) else { return (false, None) };
        let moved = |edit: &mut Edit, to: usize| {
            edit.caret = to;
            if !shift {
                edit.anchor = to;
            }
        };
        match key {
            k if k == VK_LEFT.0 => {
                let (a, b) = edit.range();
                if !shift && a != b {
                    moved(edit, a);
                } else {
                    let to = if ctrl { edit.word_left(edit.caret) } else { edit.prev(edit.caret) };
                    moved(edit, to);
                }
            }
            k if k == VK_RIGHT.0 => {
                let (a, b) = edit.range();
                if !shift && a != b {
                    moved(edit, b);
                } else {
                    let to = if ctrl { edit.word_right(edit.caret) } else { edit.next(edit.caret) };
                    moved(edit, to);
                }
            }
            k if k == VK_HOME.0 => moved(edit, 0),
            k if k == VK_END.0 => {
                let end = edit.text.len();
                moved(edit, end);
            }
            k if k == VK_BACK.0 => {
                if !edit.delete_selection() && edit.caret > 0 {
                    let from = if ctrl { edit.word_left(edit.caret) } else { edit.prev(edit.caret) };
                    edit.text.drain(from..edit.caret);
                    edit.caret = from;
                    edit.anchor = from;
                }
            }
            k if k == VK_DELETE.0 => {
                if !edit.delete_selection() && edit.caret < edit.text.len() {
                    let to = if ctrl { edit.word_right(edit.caret) } else { edit.next(edit.caret) };
                    edit.text.drain(edit.caret..to);
                    edit.anchor = edit.caret;
                }
            }
            k if ctrl && k == b'A' as u16 => {
                edit.anchor = 0;
                edit.caret = edit.text.len();
            }
            k if ctrl && (k == b'C' as u16 || k == b'X' as u16) => {
                let selected = edit.selected();
                if !selected.is_empty() && crate::clipboard::write(hwnd, &selected) && k == b'X' as u16 {
                    edit.delete_selection();
                }
            }
            k if ctrl && k == b'V' as u16 => {
                // Kotak satu baris: hanya baris pertama yang ditempel.
                if let Some(text) = pasted {
                    let first = text.lines().next().unwrap_or("");
                    let units: Vec<u16> = first.encode_utf16().collect();
                    edit.insert(&units);
                }
            }
            _ => return (false, None),
        }
        self.wake_caret();
        (true, None)
    }

    // ---- Input -------------------------------------------------------------------------------

    fn focusables(&self) -> Vec<Part> {
        let mut parts = vec![Part::Mode, Part::Search, Part::Results, Part::Earlier, Part::Later, Part::Reset, Part::More];
        if self.expanded {
            parts.extend([Part::Title, Part::Artist, Part::Save, Part::Load, Part::Saved]);
        }
        parts.retain(|p| self.enabled(*p));
        parts
    }

    /// Gulir agar kontrol yang difokus lewat keyboard terlihat (BringIntoView).
    fn reveal(&mut self, part: Part) {
        let Some(r) = self.rect_of(part) else { return };
        let height = self.client().1;
        if r.top < self.scroll {
            self.scroll = r.top;
        } else if r.bottom > self.scroll + height {
            self.scroll = r.bottom - height;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn scroll_by(&mut self, delta: f32) {
        self.scroll = (self.scroll + delta).clamp(0.0, self.max_scroll());
        self.invalidate();
    }

    fn step(&mut self, part: Part) {
        let page = self.client().1;
        match part {
            Part::Up => self.scroll_by(-SCROLL_LINE),
            Part::Down => self.scroll_by(SCROLL_LINE),
            Part::PageUp => self.scroll_by(-page),
            Part::PageDown => self.scroll_by(page),
            _ => {}
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.ensure_arranged();
        if let Some((y0, scroll0)) = self.drag {
            if let Some(bar) = self.main_bar() {
                let free = (bar.track.bottom - bar.track.top) - (bar.thumb.bottom - bar.thumb.top);
                if free > 0.0 {
                    self.scroll = (scroll0 + (y - y0) * self.max_scroll() / free).clamp(0.0, self.max_scroll());
                    self.invalidate();
                }
            }
            return;
        }
        if self.selecting {
            let position = unsafe { self.position_at(self.focus, x) };
            if let Some(edit) = self.edit_mut(self.focus) {
                edit.caret = position;
            }
            self.wake_caret();
        }
        let part = self.hit(x, y);
        if let (Part::Item(i), Some(popup)) = (part, &mut self.popup) {
            if popup.highlight != Some(i) {
                popup.highlight = Some(i);
                self.invalidate();
            }
        }
        if part != self.hover {
            self.hover = part;
            self.invalidate();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, shift: bool) {
        self.ensure_arranged();
        self.focus_visible = false;
        if self.popup.is_some() {
            let part = self.hit(x, y);
            if matches!(part, Part::Item(_)) {
                self.pressed = part;
            } else if !self.in_popup(x, y) {
                // Klik di luar menutup dropdown, seperti Popup WPF.
                self.popup = None;
            }
            self.invalidate();
            return;
        }
        let part = self.hit(x, y);
        self.pressed = part;
        unsafe {
            SetCapture(self.hwnd);
        }
        match part {
            Part::Mode | Part::Results => {
                self.set_focus(part);
                self.open_popup(part);
            }
            Part::Title | Part::Artist => {
                self.set_focus(part);
                let position = unsafe { self.position_at(part, x) };
                if let Some(edit) = self.edit_mut(part) {
                    edit.caret = position;
                    if !shift {
                        edit.anchor = position;
                    }
                }
                self.selecting = true;
            }
            Part::Thumb => self.drag = Some((y, self.scroll)),
            Part::Up | Part::Down | Part::PageUp | Part::PageDown => {
                self.step(part);
                unsafe {
                    SetTimer(self.hwnd, TIMER_REPEAT, 400, None);
                }
            }
            Part::None => {}
            _ if self.enabled(part) => self.set_focus(part),
            _ => {}
        }
        self.invalidate();
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Option<Action> {
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(self.hwnd, TIMER_REPEAT);
        }
        self.drag = None;
        self.selecting = false;
        let pressed = std::mem::replace(&mut self.pressed, Part::None);
        let part = self.hit(x, y);
        self.invalidate();
        if pressed != part || !self.enabled(part) {
            return None;
        }
        self.activate(part)
    }

    fn activate(&mut self, part: Part) -> Option<Action> {
        match part {
            Part::Search => Some(Action::Search),
            Part::Earlier => Some(Action::Earlier),
            Part::Later => Some(Action::Later),
            Part::Reset => Some(Action::ResetOffset),
            Part::Save => Some(Action::Save),
            Part::Load => Some(Action::Load),
            Part::Saved => Some(Action::Saved),
            Part::More => {
                self.expanded = !self.expanded;
                if !self.expanded && matches!(self.focus, Part::Title | Part::Artist | Part::Save | Part::Load | Part::Saved) {
                    self.set_focus(Part::More);
                }
                self.stale = true;
                None
            }
            Part::Mode | Part::Results => {
                self.open_popup(part);
                None
            }
            Part::Item(i) => {
                let combo = self.popup.take()?.combo;
                self.select(combo, i)
            }
            _ => None,
        }
    }

    fn wheel(&mut self, x: f32, y: f32, notches: f32) {
        self.ensure_arranged();
        if self.in_popup(x, y) {
            let max = self.popup_max_scroll();
            if let Some(popup) = &mut self.popup {
                popup.scroll = (popup.scroll - notches * 3.0 * SCROLL_LINE).clamp(0.0, max);
            }
            self.invalidate();
        } else if self.popup.is_none() {
            self.scroll_by(-notches * 3.0 * SCROLL_LINE);
        }
    }

    unsafe fn key_down(&mut self, key: u16, alt: bool) -> (bool, Option<Action>) {
        self.ensure_arranged();
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
        if let Some(combo) = self.popup.as_ref().map(|p| p.combo) {
            let count = self.items(combo).len();
            let Some(popup) = self.popup.as_mut() else { return (false, None) };
            match key {
                k if k == VK_UP.0 && !alt => popup.highlight = Some(popup.highlight.map_or(0, |i| i.saturating_sub(1))),
                k if k == VK_DOWN.0 && !alt => {
                    popup.highlight = Some(popup.highlight.map_or(0, |i| (i + 1).min(count.saturating_sub(1))))
                }
                k if k == VK_RETURN.0 => {
                    let highlight = popup.highlight;
                    self.popup = None;
                    self.invalidate();
                    return (true, highlight.filter(|&i| i < count).and_then(|i| self.select(combo, i)));
                }
                k if k == VK_ESCAPE.0 || k == VK_F4.0 || (alt && (k == VK_UP.0 || k == VK_DOWN.0)) || k == VK_TAB.0 => {
                    self.popup = None;
                }
                _ => return (false, None),
            }
            self.reveal_highlight();
            self.invalidate();
            return (true, None);
        }
        if key == VK_TAB.0 {
            let parts = self.focusables();
            if parts.is_empty() {
                return (true, None);
            }
            let current = parts.iter().position(|p| *p == self.focus);
            let next = match (current, shift) {
                (Some(i), false) => (i + 1) % parts.len(),
                (Some(i), true) => (i + parts.len() - 1) % parts.len(),
                (None, false) => 0,
                (None, true) => parts.len() - 1,
            };
            self.focus_visible = true;
            self.set_focus(parts[next]);
            self.reveal(parts[next]);
            return (true, None);
        }
        match self.focus {
            Part::Title | Part::Artist if !alt => self.edit_key(key, ctrl, shift),
            Part::Mode | Part::Results => {
                let combo = self.focus;
                if key == VK_F4.0 || (alt && (key == VK_DOWN.0 || key == VK_UP.0)) {
                    self.open_popup(combo);
                    self.invalidate();
                    return (true, None);
                }
                let count = self.items(combo).len();
                let current = self.selected(combo);
                let target = match key {
                    k if k == VK_UP.0 => current.map(|i| i.saturating_sub(1)),
                    k if k == VK_DOWN.0 => Some(current.map_or(0, |i| i + 1)),
                    k if k == VK_HOME.0 => Some(0),
                    k if k == VK_END.0 => Some(count.saturating_sub(1)),
                    _ => return (false, None),
                };
                self.invalidate();
                (true, target.filter(|&i| i < count).and_then(|i| self.select(combo, i)))
            }
            part if part != Part::None && !alt && (key == VK_SPACE.0 || key == VK_RETURN.0) => {
                let action = self.activate(part);
                self.invalidate();
                (true, action)
            }
            _ => {
                // Panah atas/bawah tanpa fokus: gulir isi jendela.
                match key {
                    k if k == VK_UP.0 => self.scroll_by(-SCROLL_LINE),
                    k if k == VK_DOWN.0 => self.scroll_by(SCROLL_LINE),
                    _ => return (false, None),
                }
                (true, None)
            }
        }
    }

    fn char_input(&mut self, unit: u16) {
        if unit < 32 || unit == 127 {
            return;
        }
        let part = self.focus;
        if let Some(edit) = self.edit_mut(part) {
            edit.insert(&[unit]);
            self.wake_caret();
        }
    }

    // ---- Gambar ------------------------------------------------------------------------------

    unsafe fn ensure_target(&mut self) -> Result<ID2D1HwndRenderTarget> {
        let mut r = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut r);
        let pixels = D2D_SIZE_U { width: (r.right - r.left).max(1) as u32, height: (r.bottom - r.top).max(1) as u32 };
        if let Some(target) = &self.target {
            target.Resize(&pixels)?;
            target.SetDpi(self.dpi, self.dpi);
            return Ok(target.clone());
        }
        let target = self.factory.CreateHwndRenderTarget(
            &D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: self.dpi,
                dpiY: self.dpi,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            },
            &D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd: self.hwnd, pixelSize: pixels, presentOptions: D2D1_PRESENT_OPTIONS_NONE },
        )?;
        self.target = Some(target.clone());
        Ok(target)
    }

    unsafe fn paint(&mut self) -> Result<()> {
        self.ensure_arranged();
        self.follow_caret();
        let rt = self.ensure_target()?;
        rt.BeginDraw();
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        rt.SetTransform(&translation(0.0));
        rt.Clear(Some(&self.background()));
        let drawn = self.draw(&rt);
        if rt.EndDraw(None, None).is_err() || drawn.is_err() {
            self.target = None;
        }
        Ok(())
    }

    unsafe fn draw(&self, rt: &ID2D1HwndRenderTarget) -> Result<()> {
        let (width, height) = self.client();
        let view_w = if self.bar { width - BAR_W } else { width };
        rt.PushAxisAlignedClip(&rect(0.0, 0.0, view_w, height), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        rt.SetTransform(&translation(-self.scroll));
        let white = rt.CreateSolidColorBrush(&rgba(0xFFFFFF, 1.0), None)?;
        for label in &self.labels {
            white.SetOpacity(label.alpha);
            rt.DrawTextLayout(pt(label.x, label.y), &label.layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        for &(part, r) in &self.boxes {
            match part {
                Part::Mode | Part::Results => self.draw_combo(rt, part, r)?,
                Part::Title | Part::Artist => self.draw_edit(rt, part, r)?,
                Part::More => self.draw_expander(rt, r)?,
                _ => self.draw_button(rt, part, r)?,
            }
            if self.focus_visible && self.focus == part && !matches!(part, Part::Title | Part::Artist) {
                // FocusVisualStyle WPF: garis putus-putus 1 px, masuk 2 px.
                let black = rt.CreateSolidColorBrush(&rgba(0x000000, 1.0), None)?;
                rt.DrawRectangle(&rect(r.left + 2.5, r.top + 2.5, r.right - 2.5, r.bottom - 2.5), &black, 1.0, &self.dashed);
            }
        }
        rt.SetTransform(&translation(0.0));
        rt.PopAxisAlignedClip();
        if let Some(bar) = self.main_bar() {
            let hot = if self.drag.is_some() { Part::Thumb } else { self.hover };
            self.draw_bar(rt, &bar, hot)?;
        }
        self.draw_popup(rt)
    }

    unsafe fn brush(&self, rt: &ID2D1HwndRenderTarget, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
        rt.CreateSolidColorBrush(&color, None)
    }

    unsafe fn polyline(&self, rt: &ID2D1HwndRenderTarget, points: &[(f32, f32)], brush: &ID2D1SolidColorBrush, width: f32, round: bool) -> Result<()> {
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(points[0].0, points[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
        for &(x, y) in &points[1..] {
            sink.AddLine(pt(x, y));
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        if round {
            rt.DrawGeometry(&geometry, brush, width, &self.round);
        } else {
            rt.DrawGeometry(&geometry, brush, width, None::<&ID2D1StrokeStyle>);
        }
        Ok(())
    }

    /// MusicButton: pil (CornerRadius 20) berisi NotchSoft, tepi NotchSurface, hover NotchAccent.
    unsafe fn draw_button(&self, rt: &ID2D1HwndRenderTarget, part: Part, r: D2D_RECT_F) -> Result<()> {
        let enabled = self.enabled(part);
        let opacity = if !enabled {
            0.4
        } else if self.pressed == part && self.hover == part {
            0.75
        } else {
            1.0
        };
        let hover = enabled && self.hover == part && self.popup.is_none();
        let radius = ((r.bottom - r.top) / 2.0 - 0.5).min(20.0);
        let shape = rounded(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), radius);
        rt.FillRoundedRectangle(&shape, &self.brush(rt, if hover { self.lit(opacity) } else { self.glass(opacity) })?);
        rt.DrawRoundedRectangle(&shape, &self.brush(rt, self.edge(opacity))?, 1.0, None::<&ID2D1StrokeStyle>);
        let text: Vec<u16> = self.button_text(part).encode_utf16().collect();
        let ink = self.brush(rt, rgba(0xFFFFFF, opacity))?;
        rt.DrawText(&text, &self.formats.button, &r, &ink, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        Ok(())
    }

    /// ThemedComboBox: kotak sudut 7, panah di kanan, teks pilihan dengan margin 10,0,30,0.
    unsafe fn draw_combo(&self, rt: &ID2D1HwndRenderTarget, part: Part, r: D2D_RECT_F) -> Result<()> {
        let hover = self.hover == part && self.popup.is_none();
        let shape = rounded(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 7.0);
        rt.FillRoundedRectangle(&shape, &self.brush(rt, if hover { self.lit(1.0) } else { self.glass(1.0) })?);
        rt.DrawRoundedRectangle(&shape, &self.brush(rt, self.edge(1.0))?, 1.0, None::<&ID2D1StrokeStyle>);
        let white = self.brush(rt, rgba(0xFFFFFF, 1.0))?;
        let cy = (r.top + r.bottom) / 2.0;
        let ax = r.right - 11.0 - 8.0;
        self.polyline(rt, &[(ax, cy - 2.0), (ax + 4.0, cy + 2.0), (ax + 8.0, cy - 2.0)], &white, 1.5, true)?;
        let text = match part {
            Part::Mode => MODES[self.mode].to_string(),
            _ => self.chosen.and_then(|i| self.choices.get(i)).cloned().unwrap_or_default(),
        };
        if !text.is_empty() {
            let clip = rect(r.left + 10.0, r.top, (r.right - 30.0).max(r.left + 10.0), r.bottom);
            rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let layout = self.layout_str(&text, &self.formats.normal, 100_000.0)?;
            rt.DrawTextLayout(pt(r.left + 10.0, cy - self.line / 2.0), &layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
            rt.PopAxisAlignedClip();
        }
        Ok(())
    }

    /// TextBox WPF: tepi 1 px (biru saat hover/fokus), isi NotchSoft, padding 6,4.
    unsafe fn draw_edit(&self, rt: &ID2D1HwndRenderTarget, part: Part, r: D2D_RECT_F) -> Result<()> {
        let Some(edit) = self.edit(part) else { return Ok(()) };
        let focused = self.focus == part;
        let border = if focused {
            rgba(TEXTBOX_FOCUS, 1.0)
        } else if self.hover == part && self.popup.is_none() {
            rgba(TEXTBOX_HOVER, 1.0)
        } else {
            self.edge(1.0)
        };
        rt.FillRectangle(&r, &self.brush(rt, self.glass(1.0))?);
        rt.DrawRectangle(&rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), &self.brush(rt, border)?, 1.0, None::<&ID2D1StrokeStyle>);
        let origin = r.left + 9.0 - edit.scroll;
        let top = r.top + 5.0;
        rt.PushAxisAlignedClip(&rect(r.left + 7.0, r.top + 1.0, r.right - 7.0, r.bottom - 1.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let (a, b) = edit.range();
        if focused && a != b {
            let (x1, x2) = (self.text_x(part, a), self.text_x(part, b));
            rt.FillRectangle(&rect(origin + x1, top, origin + x2, top + self.line), &self.brush(rt, rgba(SELECTION, 0.4))?);
        }
        let white = self.brush(rt, rgba(0xFFFFFF, 1.0))?;
        let layout = self.layout(&edit.text, &self.formats.normal, 100_000.0)?;
        rt.DrawTextLayout(pt(origin, top), &layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        if focused && self.caret_on {
            let x = (origin + self.text_x(part, edit.caret)).round() + 0.5;
            rt.DrawLine(pt(x, top), pt(x, top + self.line), &white, 1.0, None::<&ID2D1StrokeStyle>);
        }
        rt.PopAxisAlignedClip();
        Ok(())
    }

    /// Kepala Expander WPF (Aero2): lingkaran 19 px dengan panah, teks di kanannya.
    unsafe fn draw_expander(&self, rt: &ID2D1HwndRenderTarget, r: D2D_RECT_F) -> Result<()> {
        let hover = self.hover == Part::More && self.popup.is_none();
        let pressed = hover && self.pressed == Part::More;
        let (stroke, fill, arrow, thickness) = if pressed {
            (0x3C77DD, 0xD9ECFF, 0x000000, 1.5)
        } else if hover {
            (0x5593FF, 0xF3F9FF, 0x000000, 1.0)
        } else {
            (0x333333, 0xFFFFFF, 0x333333, 1.0)
        };
        let ellipse = D2D1_ELLIPSE { point: pt(r.left + 9.5, r.top + 9.5), radiusX: 9.5 - thickness / 2.0, radiusY: 9.5 - thickness / 2.0 };
        rt.FillEllipse(&ellipse, &self.brush(rt, rgba(fill, 1.0))?);
        rt.DrawEllipse(&ellipse, &self.brush(rt, rgba(stroke, 1.0))?, thickness, None::<&ID2D1StrokeStyle>);
        // Path 9 x 6 (termasuk tebal garis) di tengah lingkaran.
        let (x, y) = (r.left + 5.0, r.top + 6.5);
        let points = if self.expanded {
            [(x + 1.0, y + 4.5), (x + 4.5, y + 1.0), (x + 8.0, y + 4.5)]
        } else {
            [(x + 1.0, y + 1.5), (x + 4.5, y + 5.0), (x + 8.0, y + 1.5)]
        };
        self.polyline(rt, &points, &self.brush(rt, rgba(arrow, 1.0))?, 2.0, false)
    }

    unsafe fn draw_bar(&self, rt: &ID2D1HwndRenderTarget, bar: &Bar, hot: Part) -> Result<()> {
        let t = bar.track;
        rt.FillRoundedRectangle(
            &rounded(rect(t.left + 2.0, t.top + 1.0, t.right - 2.0, t.bottom - 1.0), 4.0),
            &self.brush(rt, rgba(SCROLL_TRACK.0, SCROLL_TRACK.1))?,
        );
        let thumb = if hot == Part::Thumb { self.lit(1.0) } else { self.glass(0.8) };
        rt.FillRoundedRectangle(&rounded(bar.thumb, 3.0), &self.brush(rt, thumb)?);
        for (part, cell, up) in [(Part::Up, bar.up, true), (Part::Down, bar.down, false)] {
            let opacity = if self.pressed == part && hot == part { 0.65 } else { 1.0 };
            if hot == part {
                rt.FillRoundedRectangle(&rounded(cell, 4.0), &self.brush(rt, self.edge(opacity))?);
            }
            let (cx, cy) = ((cell.left + cell.right) / 2.0, (cell.top + cell.bottom) / 2.0);
            let d = if up { -2.0 } else { 2.0 };
            let ink = self.brush(rt, self.lit(opacity))?;
            self.polyline(rt, &[(cx - 4.0, cy - d), (cx, cy + d), (cx + 4.0, cy - d)], &ink, 1.3, true)?;
        }
        Ok(())
    }

    /// Daftar dropdown: kotak NotchPopup sudut 7, padding 4, item 8,5 dengan sorotan NotchAccent.
    unsafe fn draw_popup(&self, rt: &ID2D1HwndRenderTarget) -> Result<()> {
        let (Some(popup), Some((r, _, inner))) = (&self.popup, self.popup_rect()) else { return Ok(()) };
        let shape = rounded(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 7.0);
        rt.FillRoundedRectangle(&shape, &self.brush(rt, self.popup_fill())?);
        rt.DrawRoundedRectangle(&shape, &self.brush(rt, self.edge(1.0))?, 1.0, None::<&ID2D1StrokeStyle>);
        let visible = r.bottom - r.top - 10.0;
        let scrolling = inner > visible;
        let list = rect(r.left + 5.0, r.top + 5.0, r.right - 5.0 - if scrolling { BAR_W } else { 0.0 }, r.bottom - 5.0);
        rt.PushAxisAlignedClip(&list, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let white = self.brush(rt, rgba(0xFFFFFF, 1.0))?;
        let selected = self.selected(popup.combo);
        for (i, item) in self.items(popup.combo).iter().enumerate() {
            let top = list.top + i as f32 * ITEM_H - popup.scroll;
            if top + ITEM_H < list.top || top > list.bottom {
                continue;
            }
            if popup.highlight == Some(i) {
                rt.FillRoundedRectangle(&rounded(rect(list.left, top, list.right, top + ITEM_H), 5.0), &self.brush(rt, self.lit(1.0))?);
            }
            let format = if selected == Some(i) { &self.formats.semibold } else { &self.formats.normal };
            let layout = self.layout_str(item, format, 100_000.0)?;
            rt.DrawTextLayout(pt(list.left + 8.0, top + 5.0), &layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        rt.PopAxisAlignedClip();
        if scrolling {
            let bar = Self::bar_geometry(r.right - 5.0 - 12.0, r.top + 5.0 + 2.0, r.bottom - 5.0 - 2.0, popup.scroll, visible, inner);
            self.draw_bar(rt, &bar, Part::None)?;
        }
        Ok(())
    }
}

unsafe fn size(layout: &IDWriteTextLayout) -> (f32, f32) {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    match layout.GetMetrics(&mut metrics) {
        Ok(()) => (metrics.widthIncludingTrailingWhitespace, metrics.height),
        Err(_) => (0.0, 16.0),
    }
}

fn mouse_point(lparam: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    (x / scale, y / scale)
}

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let scale = with_ui(|ui| ui.scale()).unwrap_or(1.0);
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut paint);
                with_ui(|ui| ui.paint());
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                with_ui(|ui| {
                    ui.stale = true;
                    ui.invalidate();
                });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                with_ui(|ui| {
                    ui.dpi = ((wparam.0 >> 16) & 0xFFFF) as f32;
                    ui.stale = true;
                });
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = mouse_point(lparam, scale);
                let track = with_ui(|ui| {
                    ui.mouse_move(x, y);
                    !std::mem::replace(&mut ui.tracking, true)
                });
                if track == Some(true) {
                    let mut event = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut event);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_ui(|ui| {
                    ui.tracking = false;
                    if ui.hover != Part::None && ui.pressed == Part::None {
                        ui.hover = Part::None;
                        ui.invalidate();
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = mouse_point(lparam, scale);
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                with_ui(|ui| ui.mouse_down(x, y, shift));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = mouse_point(lparam, scale);
                dispatch(with_ui(|ui| ui.mouse_up(x, y)).flatten());
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let mut point = POINT {
                    x: (lparam.0 & 0xFFFF) as u16 as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
                };
                let _ = ScreenToClient(hwnd, &mut point);
                let notches = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / 120.0;
                with_ui(|ui| ui.wheel(point.x as f32 / scale, point.y as f32 / scale, notches));
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let alt = message == WM_SYSKEYDOWN;
                let result = with_ui(|ui| ui.key_down(wparam.0 as u16, alt));
                match result {
                    Some((true, action)) => {
                        dispatch(action);
                        LRESULT(0)
                    }
                    _ => DefWindowProcW(hwnd, message, wparam, lparam),
                }
            }
            WM_CHAR => {
                with_ui(|ui| ui.char_input(wparam.0 as u16));
                LRESULT(0)
            }
            WM_TIMER => {
                with_ui(|ui| match wparam.0 {
                    TIMER_CARET => {
                        ui.caret_on = !ui.caret_on;
                        ui.invalidate();
                    }
                    TIMER_REPEAT => {
                        SetTimer(hwnd, TIMER_REPEAT, 33, None);
                        if ui.pressed == ui.hover {
                            ui.step(ui.pressed);
                        }
                    }
                    _ => {}
                });
                LRESULT(0)
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) == 1 => {
                // Kursor teks di atas kotak judul/artis (HTCLIENT).
                let text = with_ui(|ui| matches!(ui.hover, Part::Title | Part::Artist)).unwrap_or(false);
                if let Ok(cursor) = LoadCursorW(None, if text { IDC_IBEAM } else { IDC_ARROW }) {
                    SetCursor(cursor);
                }
                LRESULT(1)
            }
            WM_KILLFOCUS => {
                with_ui(|ui| {
                    ui.popup = None;
                    ui.caret_on = false;
                    let _ = KillTimer(hwnd, TIMER_CARET);
                    ui.invalidate();
                });
                LRESULT(0)
            }
            WM_SETFOCUS => {
                with_ui(|ui| ui.wake_caret());
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                info.ptMinTrackSize.x = (MIN_WIDTH * dpi) as i32;
                info.ptMinTrackSize.y = (MIN_HEIGHT * dpi) as i32;
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(hwnd, TIMER_CARET);
                let _ = KillTimer(hwnd, TIMER_REPEAT);
                UI.with(|cell| {
                    if let Ok(mut ui) = cell.try_borrow_mut() {
                        ui.take();
                    }
                });
                dispatch(Some(Action::Closed));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}
