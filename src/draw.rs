// Menggambar notch dengan Direct2D ke swap chain DirectComposition (premultiplied alpha): GPU langsung
// menampilkannya tersinkron refresh monitor, tanpa salinan per frame seperti jendela layered.
// Klik tembus di luar notch lewat region jendela (SetWindowRgn) seukuran bentuk notch.
// Semua koordinat dalam DIP; tata letak mengikuti MainWindow.xaml NotchMusic.

use std::mem::ManuallyDrop;

use windows::core::{w, Interface, Result};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING, D2D1_PIXEL_FORMAT,
    D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory, ID2D1Factory1, ID2D1Geometry, ID2D1Image,
    ID2D1RenderTarget, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_CAP_STYLE_ROUND,
    D2D1_CAP_STYLE_FLAT, D2D1_COMBINE_MODE_EXCLUDE, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_ROUND, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIAdapter, IDXGIDevice, IDXGIDevice1, IDXGIFactory2, IDXGIOutput, IDXGISurface, IDXGISwapChain1,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER};

use crate::media::MediaState;

// Ukuran notch (DIP), sama dengan SmallWidth/SmallHeight dan LargeWidth + 2 x MaxFlare/LargeHeight.
pub const COMPACT_W: f32 = 240.0;
pub const COMPACT_H: f32 = 34.0;
pub const EXPANDED_W: f32 = 648.0;
pub const EXPANDED_H: f32 = 160.0;
const FLARE: f32 = 14.0;
const CENTER_X: f32 = EXPANDED_W / 2.0;
const KAPPA: f32 = 0.552_284_8; // Pendekatan seperempat lingkaran dengan Bezier kubik.

const LOGO_COLOR: u32 = 0x7DE2B4;
const TEXT: u32 = 0xFFFFFF;
const LYRIC_BOX: (u32, f32) = (0x8E9162, 0x26 as f32 / 255.0);

/// Background, Surface, Soft, Accent, Ink: sama dengan tema bawaan NotchMusic.
pub struct Theme {
    pub name: &'static str,
    pub background: u32,
    pub surface: u32,
    pub soft: u32,
    pub accent: u32,
    pub ink: u32,
}

pub const THEMES: [Theme; 6] = [
    Theme { name: "Zaitun", background: 0x6A795D, surface: 0x8E9162, soft: 0xB2BD7E, accent: 0xD9D28B, ink: 0x182116 },
    Theme { name: "Malam", background: 0x1E2A44, surface: 0x34466B, soft: 0x7FA3D9, accent: 0xBFD7FF, ink: 0x0B1222 },
    Theme { name: "Grafit", background: 0x1C1C1E, surface: 0x3A3A3C, soft: 0xA1A1A6, accent: 0xE5E5EA, ink: 0x111113 },
    Theme { name: "Anggur", background: 0x4A2433, surface: 0x6E3A4D, soft: 0xC98AA0, accent: 0xF2C4CE, ink: 0x22101A },
    Theme { name: "Biru Es", background: 0x112D4E, surface: 0x3F72AF, soft: 0xDBE2EF, accent: 0xF9F7F7, ink: 0x112D4E },
    // Hitam legam: latar #000 menyatu dengan bezel layar; aksen mint senada logo.
    Theme { name: "Hitam Legam", background: 0x000000, surface: 0x2C2C2E, soft: 0xF2F2F7, accent: 0x7DE2B4, ink: 0x000000 },
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    None,
    Previous,
    PlayPause,
    Next,
    Seek(f32),
    Pin,
    Theme,
    Minimize,
    PageLeft,
    PageRight,
    Lyrics,
    ClipPause,
    ClipReset,
    ClipSearch,
    ClipRow(u64),
    ClipCopy(u64),
    ClipDelete(u64),
    PrayerCity,
}

pub const PAGE_MUSIC: usize = 0;
pub const PAGE_CLIPBOARD: usize = 1;
pub const PAGE_PRAYER: usize = 2;
pub const PAGE_COUNT: usize = 3;

/// Halaman jadwal salat (lihat prayer.rs).
pub struct PrayerView<'a> {
    pub city: &'a str,
    pub zone: &'static str,
    pub times: [crate::prayer::PrayerTime; 6],
    /// Menit sejak tengah malam waktu kota.
    pub now: f64,
    /// Indeks waktu berikutnya hari ini (None = Subuh besok), waktunya, dan hitung mundur.
    pub next: Option<usize>,
    pub next_time: crate::prayer::PrayerTime,
    pub countdown: &'a str,
}

pub struct ClipRow<'a> {
    pub id: u64,
    pub preview: &'a str,
    pub time: &'a str,
}

pub struct ClipView<'a> {
    pub rows: Vec<ClipRow<'a>>,
    pub total: usize,
    pub recording: bool,
    pub search: &'a str,
    pub focused: bool,
    pub caret: bool,
    pub status: &'a str,
    pub scroll: f32,
}

pub struct LyricView<'a> {
    pub previous: &'a str,
    pub current: &'a str,
    pub next: &'a str,
    /// Baris lirik untuk notch kecil (lagu sedang diputar dan lirik berwaktu tersedia).
    pub compact: Option<&'a str>,
    /// Tinggi bar equalizer 0..1.
    pub eq: [f32; 5],
}

pub struct View<'a> {
    /// 0 = notch kecil, 1 = notch terbuka (sudah di-ease).
    pub t: f32,
    /// Ukuran notch kecil saat ini (melebar untuk lirik).
    pub compact: (f32, f32),
    pub clock: &'a str,
    pub date: &'a str,
    pub down: &'a str,
    pub up: &'a str,
    pub media: &'a MediaState,
    pub now: i64,
    pub hover: Hit,
    pub theme: &'a Theme,
    pub pinned: bool,
    pub lyric: LyricView<'a>,
    pub clip: ClipView<'a>,
    pub page: usize,
    /// Halaman lama yang sedang bergeser keluar, arahnya (-1/1), dan progres 0..1.
    pub transition: Option<(usize, f32, f32)>,
    pub prayer: PrayerView<'a>,
    /// Pengingat singkat di notch kecil (mis. waktu salat), menggantikan jam/lirik.
    pub notice: Option<&'a str>,
}

// Halaman salat: judul + kota 24 | kartu berikutnya 128 | 6 | lintasan matahari 388 x 86.
const PRAYER_CITY: (f32, f32) = (PAGE_RIGHT - 150.0, PAGE_RIGHT);
const PRAYER_HEADER_Y: f32 = 19.0;
const PRAYER_CARD: (f32, f32, f32, f32) = (PAGE_LEFT, 38.0, PAGE_LEFT + 128.0, 128.0);
const SKY_LEFT: f32 = PAGE_LEFT + 134.0;
const SKY_TOP: f32 = 40.0;
const SKY_W: f32 = PAGE_RIGHT - SKY_LEFT;
const SKY_H: f32 = 86.0;
const SUN: u32 = 0xFFD56B;

// Tata letak panel terbuka (koordinat jendela, DIP). Deck 594 x 142 dimulai di (27, 8):
// kolom panah 28 | 8 | halaman 522 | 8 | panah 28. Halaman musik: pemutar 292 | 18 | lirik 212.
const PAGE_LEFT: f32 = 63.0;
const PLAYER_RIGHT: f32 = 355.0;
const LYRICS_LEFT: f32 = 373.0;
const LYRICS_RIGHT: f32 = 585.0;
const ARROW_LEFT_X: f32 = 41.0;
const ARROW_RIGHT_X: f32 = 607.0;
const ROW_SEEK_Y: f32 = 77.0;
const ROW_BUTTONS_Y: f32 = 114.0;
const CONTROLS_X: f32 = (PAGE_LEFT + PLAYER_RIGHT) / 2.0;
const PREVIOUS_X: f32 = CONTROLS_X - 42.0;
const NEXT_X: f32 = CONTROLS_X + 42.0;
const PIN_X: f32 = 74.0;
const THEME_X: f32 = 99.0;
const MINIMIZE_X: f32 = 124.0;
const LYRICS_BUTTON_X: f32 = 343.0;
const PAGE_RIGHT: f32 = 585.0;
const PAGE_WIDTH: f32 = PAGE_RIGHT - PAGE_LEFT;
const ARROW_Y: f32 = 79.0;

// Halaman clipboard: judul 22 | cari 28 | daftar * | status 16.
const CLIP_HEADER_Y: f32 = 19.0;
const CLIP_RESET: (f32, f32) = (537.0, PAGE_RIGHT);
const CLIP_PAUSE: (f32, f32) = (485.0, 533.0);
const CLIP_SEARCH_TOP: f32 = 32.0;
const CLIP_SEARCH_BOTTOM: f32 = 54.0;
const CLIP_LIST_TOP: f32 = 58.0;
const CLIP_LIST_BOTTOM: f32 = 126.0;
pub const CLIP_ROW_H: f32 = 30.0;
pub const CLIP_LIST_H: f32 = CLIP_LIST_BOTTOM - CLIP_LIST_TOP;

// Notch kecil mode lirik: inset kiri-kanan 32, kolom equalizer 30, jarak 12 (CompactChrome = 112).
const COMPACT_INSET: f32 = 32.0;
pub const COMPACT_CHROME: f32 = COMPACT_INSET * 2.0 + 30.0 + 12.0 + 6.0;
pub const MUSIC_H: f32 = 42.0;

pub fn shape_bounds(t: f32, compact: (f32, f32)) -> (f32, f32, f32) {
    let w = lerp(compact.0, EXPANDED_W, t);
    let h = lerp(compact.1, EXPANDED_H, t);
    ((EXPANDED_W - w) / 2.0, w, h)
}

/// Apakah titik (DIP) berada di dalam bentuk notch saat ini.
pub fn inside_shape(t: f32, compact: (f32, f32), x: f32, y: f32) -> bool {
    let (x0, w, h) = shape_bounds(t, compact);
    x >= x0 && x <= x0 + w && y >= 0.0 && y <= h
}

/// Lebar daftar clipboard: menyisakan tempat scrollbar bila isi lebih tinggi dari area daftar.
fn clip_list_right(rows: usize) -> f32 {
    if rows as f32 * CLIP_ROW_H > CLIP_LIST_H { PAGE_RIGHT - 16.0 } else { PAGE_RIGHT }
}

/// Kolom baris clipboard: pratinjau * | jam 38 | salin 28 | hapus 24 (padding 5).
fn clip_columns(right: f32) -> (f32, f32, f32) {
    let delete_x = right - 5.0 - 12.0;
    let copy_x = right - 5.0 - 24.0 - 14.0;
    let time_x = right - 5.0 - 24.0 - 28.0 - 19.0;
    (time_x, copy_x, delete_x)
}

/// Batas slider; lebar teks waktu ikut menentukan, seperti kolom Auto di XAML.
fn seek_track(elapsed_width: f32, duration_width: f32) -> (f32, f32) {
    (PAGE_LEFT + elapsed_width + 8.0, PLAYER_RIGHT - duration_width - 8.0)
}

fn near(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> bool {
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// Yang dibutuhkan hit-test dari halaman clipboard: id baris yang tampil (setelah pencarian) dan posisi gulir.
pub struct ClipHit<'a> {
    pub ids: &'a [u64],
    pub total: usize,
    pub scroll: f32,
}

pub fn hit_test(t: f32, page: usize, media: &MediaState, track: (f32, f32), clip: &ClipHit, x: f32, y: f32) -> Hit {
    if t < 0.95 {
        return Hit::None;
    }
    if near(x, y, ARROW_LEFT_X, ARROW_Y, 12.0) {
        return Hit::PageLeft;
    }
    if near(x, y, ARROW_RIGHT_X, ARROW_Y, 12.0) {
        return Hit::PageRight;
    }
    if page == PAGE_CLIPBOARD {
        return clip_hit(clip, x, y);
    }
    if page == PAGE_PRAYER {
        let (l, r) = PRAYER_CITY;
        return if x >= l && x <= r && (y - PRAYER_HEADER_Y).abs() <= 11.0 { Hit::PrayerCity } else { Hit::None };
    }
    if near(x, y, PIN_X, ROW_BUTTONS_Y, 11.0) {
        return Hit::Pin;
    }
    if near(x, y, THEME_X, ROW_BUTTONS_Y, 11.0) {
        return Hit::Theme;
    }
    if near(x, y, MINIMIZE_X, ROW_BUTTONS_Y, 11.0) {
        return Hit::Minimize;
    }
    if !media.active {
        return Hit::None;
    }
    if near(x, y, LYRICS_BUTTON_X, ROW_BUTTONS_Y, 12.0) {
        return Hit::Lyrics;
    }
    let (left, right) = track;
    if media.duration_secs() > 0.0 && x >= left && x <= right && (y - ROW_SEEK_Y).abs() <= 11.0 {
        let (start, end) = (left + 8.0, right - 8.0); // Thumb 16 DIP: pusatnya tidak keluar track.
        return Hit::Seek(((x - start) / (end - start)).clamp(0.0, 1.0));
    }
    if near(x, y, PREVIOUS_X, ROW_BUTTONS_Y, 14.0) {
        Hit::Previous
    } else if near(x, y, CONTROLS_X, ROW_BUTTONS_Y, 17.0) {
        Hit::PlayPause
    } else if near(x, y, NEXT_X, ROW_BUTTONS_Y, 14.0) {
        Hit::Next
    } else {
        Hit::None
    }
}

fn clip_hit(clip: &ClipHit, x: f32, y: f32) -> Hit {
    let chip = |(l, r): (f32, f32)| x >= l && x <= r && (y - CLIP_HEADER_Y).abs() <= 10.0;
    if chip(CLIP_PAUSE) {
        return Hit::ClipPause;
    }
    if chip(CLIP_RESET) && clip.total > 0 {
        return Hit::ClipReset;
    }
    if x >= PAGE_LEFT && x <= PAGE_RIGHT && (CLIP_SEARCH_TOP..=CLIP_SEARCH_BOTTOM).contains(&y) {
        return Hit::ClipSearch;
    }
    let right = clip_list_right(clip.ids.len());
    if !(CLIP_LIST_TOP..CLIP_LIST_BOTTOM).contains(&y) || x < PAGE_LEFT || x > right {
        return Hit::None;
    }
    let index = ((y - CLIP_LIST_TOP + clip.scroll) / CLIP_ROW_H) as usize;
    let Some(&id) = clip.ids.get(index) else { return Hit::None };
    let center_y = CLIP_LIST_TOP - clip.scroll + index as f32 * CLIP_ROW_H + CLIP_ROW_H / 2.0;
    let (_, copy_x, delete_x) = clip_columns(right);
    if near(x, y, copy_x, center_y, 11.0) {
        Hit::ClipCopy(id)
    } else if near(x, y, delete_x, center_y, 9.0) {
        Hit::ClipDelete(id)
    } else {
        Hit::ClipRow(id)
    }
}

struct Formats {
    compact_clock: IDWriteTextFormat,
    speed: IDWriteTextFormat,
    small_bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    title: IDWriteTextFormat,
    artist: IDWriteTextFormat,
    time_left: IDWriteTextFormat,
    time_right: IDWriteTextFormat,
    lyric: IDWriteTextFormat,
    lyric_side: IDWriteTextFormat,
    compact_lyric: IDWriteTextFormat,
    clip_title: IDWriteTextFormat,
    clip_count: IDWriteTextFormat,
    chip: IDWriteTextFormat,
    search: IDWriteTextFormat,
    row: IDWriteTextFormat,
    row_time: IDWriteTextFormat,
    note_center: IDWriteTextFormat,
    status: IDWriteTextFormat,
    sky_name: IDWriteTextFormat,
    sky_time: IDWriteTextFormat,
    card_label: IDWriteTextFormat,
    card_name: IDWriteTextFormat,
    card_time: IDWriteTextFormat,
    card_note: IDWriteTextFormat,
}

/// Ikon dari MainWindow.xaml, semuanya di bidang 20 x 20.
struct Icons {
    logo: ID2D1Geometry,
    play: ID2D1PathGeometry,
    pause: ID2D1PathGeometry,
    previous: ID2D1PathGeometry,
    next: ID2D1PathGeometry,
    note: ID2D1PathGeometry,
    pin: ID2D1PathGeometry,
    pin_slash: ID2D1PathGeometry,
    theme_half: ID2D1PathGeometry,
    minimize: ID2D1PathGeometry,
    lyrics: ID2D1PathGeometry,
    arrow_left: ID2D1PathGeometry,
    arrow_right: ID2D1PathGeometry,
    copy: ID2D1PathGeometry,
    close: ID2D1PathGeometry,
    chevron: ID2D1PathGeometry,
}

/// Perangkat GPU + rantai tampilan DirectComposition. Dibuat ulang utuh bila GPU di-reset.
struct Gpu {
    d3d: ID3D11Device,
    dxgi_factory: IDXGIFactory2,
    dcomp: IDCompositionDevice,
    context: ID2D1DeviceContext,
    swap: Option<IDXGISwapChain1>,
    /// Target komposisi milik jendela; disimpan agar visual tetap tampil.
    composition: Option<(IDCompositionTarget, IDCompositionVisual)>,
}

pub struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    gpu: Gpu,
    /// DeviceContext yang sama, dilihat sebagai render target biasa agar kode gambar tetap sederhana.
    target: ID2D1RenderTarget,
    formats: Formats,
    icons: Icons,
    round: ID2D1StrokeStyle,
    /// Garis putus-putus busur siang dan garis titik penghubung label (halaman salat).
    dash: ID2D1StrokeStyle,
    dot: ID2D1StrokeStyle,
    width: i32,
    height: i32,
    dpi: f32,
    /// Posisi/ukuran jendela dan region klik terakhir, agar Win32 hanya dipanggil saat berubah.
    placed: (i32, i32, i32, i32),
    region: (i32, i32, i32, i32),
    cover: Option<(u64, ID2D1Bitmap)>,
    /// Batas slider terakhir yang digambar, untuk hit-test.
    pub track: (f32, f32),
}

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let semi = DWRITE_FONT_WEIGHT_SEMI_BOLD;
            let normal = DWRITE_FONT_WEIGHT_NORMAL;
            let formats = Formats {
                compact_clock: format(&dwrite, 15.0, semi, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                speed: format(&dwrite, 10.5, semi, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                small_bold: format(&dwrite, 10.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                small: format(&dwrite, 10.0, normal, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
                title: format(&dwrite, 14.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
                artist: format(&dwrite, 11.0, normal, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
                time_left: format(&dwrite, 12.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                time_right: format(&dwrite, 12.0, semi, DWRITE_TEXT_ALIGNMENT_TRAILING, false, false)?,
                lyric: format(&dwrite, 14.0, semi, DWRITE_TEXT_ALIGNMENT_CENTER, false, true)?,
                lyric_side: format(&dwrite, 11.0, normal, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                compact_lyric: format(&dwrite, 16.0, semi, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                clip_title: format(&dwrite, 12.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                clip_count: format(&dwrite, 10.0, normal, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                chip: format(&dwrite, 10.0, semi, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                search: format(&dwrite, 12.0, normal, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
                row: format(&dwrite, 11.0, normal, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
                row_time: format(&dwrite, 9.0, normal, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                note_center: format(&dwrite, 11.0, normal, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                status: format(&dwrite, 10.0, normal, DWRITE_TEXT_ALIGNMENT_CENTER, true, false)?,
                sky_name: format(&dwrite, 9.0, normal, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                sky_time: format(&dwrite, 12.0, semi, DWRITE_TEXT_ALIGNMENT_CENTER, false, false)?,
                card_label: format(&dwrite, 9.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                card_name: format(&dwrite, 13.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                card_time: format(&dwrite, 22.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING, false, false)?,
                card_note: format(&dwrite, 11.0, semi, DWRITE_TEXT_ALIGNMENT_LEADING, true, false)?,
            };
            let icons = Icons {
                logo: logo_geometry(&factory)?,
                play: path(&factory, "M6,3 L18,10 L6,17 Z", true)?,
                pause: path(&factory, "M5,3 H9 V17 H5 Z M11,3 H15 V17 H11 Z", true)?,
                previous: path(&factory, "M3,3 H5 V17 H3 Z M17,3 V17 L7,10 Z", true)?,
                next: path(&factory, "M15,3 H17 V17 H15 Z M3,3 L13,10 L3,17 Z", true)?,
                note: path(&factory, "M8,15 V5 L17,3 V13 M8,8 L17,6", false)?,
                pin: path(&factory, "M6,3 H14 M7,3 V9 L4.5,12 H15.5 L13,9 V3 M10,12 V18", false)?,
                pin_slash: path(&factory, "M3,3 L17,17", false)?,
                theme_half: path(&factory, "M10,2 A8,8 0 0 1 10,18 Z", true)?,
                minimize: path(&factory, "M4,10 H16", false)?,
                lyrics: path(
                    &factory,
                    "M2,15.5 L6,4.5 L10,15.5 M3.5,11.5 H8.5 M17.5,11.5 C17.5,7.5 12,7.5 12,11.5 C12,15.5 17.5,15.5 17.5,11.5 M17.5,8.5 V15.5",
                    false,
                )?,
                arrow_left: path(&factory, "M13,4 L7,10 L13,16", false)?,
                arrow_right: path(&factory, "M7,4 L13,10 L7,16", false)?,
                copy: path(&factory, "M7,3 H16 V13 H7 Z M4,7 H3 V17 H12 V16", false)?,
                close: path(&factory, "M5,5 L15,15 M15,5 L5,15", false)?,
                chevron: path(&factory, "M5,8 L10,13 L15,8", false)?,
            };
            let round = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let dashed = |dashes: &[f32]| {
                factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_FLAT,
                        endCap: D2D1_CAP_STYLE_FLAT,
                        dashCap: D2D1_CAP_STYLE_FLAT,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        dashStyle: D2D1_DASH_STYLE_CUSTOM,
                        dashOffset: 0.0,
                    },
                    Some(dashes),
                )
            };
            let dash = dashed(&[1.3, 2.0])?;
            let dot = dashed(&[1.5, 2.5])?;
            let gpu = create_gpu(&factory)?;
            let target = gpu.context.cast()?;
            Ok(Gfx {
                factory,
                dwrite,
                wic,
                gpu,
                target,
                formats,
                icons,
                round,
                dash,
                dot,
                width: 0,
                height: 0,
                dpi: 96.0,
                placed: (0, 0, 0, 0),
                region: (0, 0, 0, 0),
                cover: None,
                track: seek_track(30.0, 30.0),
            })
        }
    }

    /// Ukuran jendela (piksel) dan DPI monitor; swap chain menyesuaikan saat frame berikutnya.
    pub fn resize(&mut self, width: i32, height: i32, dpi: f32) -> Result<()> {
        unsafe {
            self.width = width;
            self.height = height;
            self.dpi = dpi;
            if let Some(swap) = &self.gpu.swap {
                self.gpu.context.SetTarget(None::<&ID2D1Image>);
                swap.ResizeBuffers(2, width as u32, height as u32, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            }
            Ok(())
        }
    }

    pub fn present(&mut self, hwnd: HWND, origin: POINT, view: &View, dpi: f32) -> Result<()> {
        unsafe {
            self.dpi = dpi;
            self.place(hwnd, origin, view);
            if self.frame(hwnd, view).is_err() {
                // GPU di-reset (driver diperbarui, dsb.): buat ulang semua perangkat lalu coba sekali lagi.
                self.gpu = create_gpu(&self.factory)?;
                self.target = self.gpu.context.cast()?;
                self.cover = None;
                self.frame(hwnd, view)?;
            }
            Ok(())
        }
    }

    /// Jendela tetap seukuran notch terbuka; region dibatasi ke kotak bentuk notch saat ini,
    /// jadi klik di area transparan tembus ke aplikasi di belakang.
    unsafe fn place(&mut self, hwnd: HWND, origin: POINT, view: &View) {
        let window = (origin.x, origin.y, self.width, self.height);
        if window != self.placed {
            let _ = SetWindowPos(hwnd, HWND_TOP, window.0, window.1, window.2, window.3, SWP_NOACTIVATE | SWP_NOZORDER);
            self.placed = window;
        }
        let scale = self.dpi / 96.0;
        let (x0, w, h) = shape_bounds(view.t, view.compact);
        let region = (
            (x0 * scale).floor() as i32,
            0,
            ((x0 + w) * scale).ceil() as i32,
            (h * scale).ceil() as i32 + 1,
        );
        if region != self.region {
            let handle = CreateRectRgn(region.0, region.1, region.2, region.3);
            // Berhasil = region jadi milik Windows; redraw tidak perlu (isi digambar lewat komposisi).
            if SetWindowRgn(hwnd, handle, false) == 0 {
                let _ = DeleteObject(handle);
            }
            self.region = region;
        }
    }

    unsafe fn frame(&mut self, hwnd: HWND, view: &View) -> Result<()> {
        if self.width <= 0 || self.height <= 0 {
            return Ok(());
        }
        let gpu = &mut self.gpu;
        if gpu.swap.is_none() {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: self.width as u32,
                Height: self.height as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            gpu.swap = Some(gpu.dxgi_factory.CreateSwapChainForComposition(&gpu.d3d, &desc, None::<&IDXGIOutput>)?);
        }
        let swap = gpu.swap.clone().unwrap();
        if gpu.composition.is_none() {
            let target = gpu.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = gpu.dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            gpu.dcomp.Commit()?;
            gpu.composition = Some((target, visual));
        }
        let surface: IDXGISurface = swap.GetBuffer(0)?;
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: self.dpi,
            dpiY: self.dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let bitmap = gpu.context.CreateBitmapFromDxgiSurface(&surface, Some(&properties))?;
        gpu.context.SetTarget(&bitmap);
        gpu.context.SetDpi(self.dpi, self.dpi);
        gpu.context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let painted = self.paint(view);
        self.gpu.context.SetTarget(None::<&ID2D1Image>);
        painted?;
        // Sinkron ke vsync: satu frame per refresh monitor, sekaligus menjadi pengatur tempo animasi.
        swap.Present(1, DXGI_PRESENT(0)).ok()
    }
    unsafe fn paint(&mut self, v: &View) -> Result<()> {
        self.update_cover(v.media);
        let rt = self.target.clone();
        rt.BeginDraw();
        rt.SetTransform(&Matrix3x2::identity());
        rt.Clear(Some(&color(0, 0.0)));

        let theme = v.theme;
        let (x0, w, h) = shape_bounds(v.t, v.compact);
        let shape = self.notch_shape(x0, w, h)?;
        rt.FillGeometry(&shape, &self.brush(theme.background, 1.0)?, None::<&ID2D1Brush>);

        // Garis tepi dan garis progres dipotong oleh bentuk notch, seperti clip di NotchSurface.
        let layer = rt.CreateLayer(None)?;
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
            geometricMask: ManuallyDrop::new(Some(shape.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        rt.PushLayer(&params, &layer);
        drop(ManuallyDrop::into_inner(params.geometricMask));
        rt.DrawGeometry(&shape, &self.brush(theme.surface, 1.0)?, 1.2, None::<&ID2D1StrokeStyle>);

        let media = v.media;
        let duration = media.duration_secs();
        let elapsed = media.elapsed_secs(v.now);
        let fraction = if duration > 0.0 { (elapsed / duration) as f32 } else { 0.0 };
        let compact_alpha = 1.0 - (v.t * 2.5).min(1.0);
        if compact_alpha > 0.0 && media.active && duration > 0.0 {
            self.paint_edge_progress(x0, w, h, fraction, &self.brush(theme.accent, compact_alpha)?)?;
        }
        rt.PopLayer();

        if compact_alpha > 0.0 {
            match (v.notice, v.lyric.compact) {
                (Some(notice), _) => {
                    let cy = h / 2.0 - 1.0;
                    let area = rect(x0 + COMPACT_INSET, cy - 12.0, x0 + w - COMPACT_INSET, cy + 12.0);
                    self.text_fit(notice, &self.formats.compact_lyric, area, &self.brush(TEXT, compact_alpha)?, Matrix3x2::identity())?;
                }
                (None, Some(line)) => self.paint_compact_lyric(v, line, x0, w, h, compact_alpha)?,
                (None, None) => self.paint_compact(v, compact_alpha)?,
            }
        }
        let alpha = ((v.t - 0.35) / 0.65).clamp(0.0, 1.0);
        if alpha > 0.0 {
            // Isi panel tumbuh dari 94% ke 100% dari tengah atas, seperti ExpandedScale.
            let s = 0.94 + 0.06 * alpha;
            let base = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: CENTER_X * (1.0 - s), M32: 8.0 * (1.0 - s) };
            rt.SetTransform(&base);
            self.paint_deck(v, alpha, base)?;
            // Halaman bergeser di dalam viewport 522 DIP; panah dan logo tetap di tempat.
            rt.PushAxisAlignedClip(&rect(PAGE_LEFT, 0.0, PAGE_RIGHT, EXPANDED_H), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let pages: Vec<(usize, f32, f32)> = match v.transition {
                Some((from, direction, p)) => vec![
                    (from, -direction * PAGE_WIDTH * p, alpha * (1.0 - p)),
                    (v.page, direction * PAGE_WIDTH * (1.0 - p), alpha * p),
                ],
                None => vec![(v.page, 0.0, alpha)],
            };
            for (page, dx, page_alpha) in pages {
                if page_alpha <= 0.0 {
                    continue;
                }
                let shifted = multiply(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: 0.0 }, &base);
                rt.SetTransform(&shifted);
                if page == PAGE_CLIPBOARD {
                    self.paint_clipboard(v, page_alpha, shifted)?;
                } else if page == PAGE_PRAYER {
                    self.paint_prayer(v, page_alpha, shifted)?;
                } else {
                    self.paint_music(v, page_alpha, elapsed, duration, fraction, shifted)?;
                }
            }
            rt.SetTransform(&base);
            rt.PopAxisAlignedClip();
            rt.SetTransform(&Matrix3x2::identity());
        }
        rt.EndDraw(None, None)
    }

    /// Bentuk NotchSurface saat menempel di atas layar: sudut atas melengkung ke luar (flare),
    /// sudut bawah membulat.
    unsafe fn notch_shape(&self, x: f32, w: f32, h: f32) -> Result<ID2D1PathGeometry> {
        let (y, r, d) = (0.0, x + w, h);
        let flare = FLARE.min(w / 6.0);
        let (left, right) = (x + flare, r - flare);
        let radius = 24f32.min(((right - left) / 2.0).min(h / 2.0));
        let lower = (h - flare).min(((right - left) / 2.0).min(radius * 2.0));
        let k = KAPPA;
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(pt(r, y));
        sink.AddBezier(&bezier((r - k * flare, y), (right, y + flare - k * flare), (right, y + flare)));
        sink.AddLine(pt(right, d - lower));
        sink.AddBezier(&bezier((right, d - lower + k * lower), (right - lower + k * lower, d), (right - lower, d)));
        sink.AddLine(pt(left + lower, d));
        sink.AddBezier(&bezier((left + lower - k * lower, d), (left, d - lower + k * lower), (left, d - lower)));
        sink.AddLine(pt(left, y + flare));
        sink.AddBezier(&bezier((left, y + flare - k * flare), (x + k * flare, y), (x, y)));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        Ok(geometry)
    }

    /// Garis progres di tepi notch kecil: dari tengah bawah naik ke kiri dan kanan sekaligus,
    /// penuh tepat di ujung lengkung atas saat lagu selesai.
    unsafe fn paint_edge_progress(&self, x: f32, w: f32, h: f32, fraction: f32, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let (y, r, d) = (0.0, x + w, h);
        let flare = FLARE.min(w / 6.0);
        let (left, right) = (x + flare, r - flare);
        let radius = 24f32.min(((right - left) / 2.0).min(h / 2.0));
        let lower = (h - flare).min(((right - left) / 2.0).min(radius * 2.0));
        let center = (x + r) / 2.0;
        let k = KAPPA;
        // Separuh kanan sebagai titik-titik (Bezier disampel), lalu dicerminkan untuk kiri.
        let mut points = vec![(center, d), (right - lower, d)];
        sample_bezier(&mut points, (right - lower, d), (right - lower + k * lower, d), (right, d - lower + k * lower), (right, d - lower));
        points.push((right, y + flare));
        sample_bezier(&mut points, (right, y + flare), (right, y + flare - k * flare), (r - k * flare, y), (r, y));
        let total: f32 = points.windows(2).map(|p| dist(p[0], p[1])).sum();
        let mut remaining = total * fraction.clamp(0.0, 1.0);
        if remaining <= 0.0 {
            return Ok(());
        }
        let mut visible = vec![points[0]];
        for pair in points.windows(2) {
            let length = dist(pair[0], pair[1]);
            if remaining >= length {
                visible.push(pair[1]);
                remaining -= length;
            } else {
                let t = remaining / length;
                visible.push((lerp(pair[0].0, pair[1].0, t), lerp(pair[0].1, pair[1].1, t)));
                break;
            }
        }
        for mirror in [false, true] {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let map = |(px, py): (f32, f32)| if mirror { pt(2.0 * center - px, py) } else { pt(px, py) };
            sink.BeginFigure(map(visible[0]), D2D1_FIGURE_BEGIN_HOLLOW);
            for p in &visible[1..] {
                sink.AddLine(map(*p));
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            // Setengah tebal garis terpotong clip, jadi yang tampak sekitar 1,5 DIP.
            self.target.DrawGeometry(&geometry, brush, 3.0, None::<&ID2D1StrokeStyle>);
        }
        Ok(())
    }

    /// Notch kecil: jam tepat di tengah, logo di kirinya, kecepatan internet di kanannya.
    unsafe fn paint_compact(&self, v: &View, alpha: f32) -> Result<()> {
        let white = self.brush(TEXT, alpha)?;
        let clock_width = self.measure(v.clock, &self.formats.compact_clock);
        let (cy, half) = (16.0, clock_width / 2.0);
        self.text(v.clock, &self.formats.compact_clock, rect(CENTER_X - half - 4.0, 4.0, CENTER_X + half + 4.0, 28.0), &white);
        let logo_right = CENTER_X - half - 8.0;
        self.fill_icon_box(&self.icons.logo, logo_right - 14.0, cy - 7.0, 14.0, 100.0, &self.brush(LOGO_COLOR, alpha)?, Matrix3x2::identity());
        let speed_left = CENTER_X + half + 8.0;
        self.text(&format!("↓ {}", v.down), &self.formats.speed, rect(speed_left, cy - 12.0, speed_left + 60.0, cy), &white);
        self.text(&format!("↑ {}", v.up), &self.formats.speed, rect(speed_left, cy, speed_left + 60.0, cy + 12.0), &white);
        Ok(())
    }

    /// Notch kecil saat lagu diputar dengan lirik berwaktu: baris lirik + equalizer di kanan.
    unsafe fn paint_compact_lyric(&self, v: &View, line: &str, x0: f32, w: f32, h: f32, alpha: f32) -> Result<()> {
        let cy = h / 2.0 - 1.0;
        let left = x0 + COMPACT_INSET;
        let eq_right = x0 + w - COMPACT_INSET;
        let eq_left = eq_right - 30.0;
        let text_right = eq_left - 12.0;
        self.text_fit(line, &self.formats.compact_lyric, rect(left, cy - 12.0, text_right, cy + 12.0), &self.brush(TEXT, alpha)?, Matrix3x2::identity())?;
        let accent = self.brush(v.theme.accent, alpha)?;
        for (i, scale) in v.lyric.eq.iter().enumerate() {
            let x = eq_left + 1.5 + i as f32 * 6.0;
            let half = 9.0 * scale.clamp(0.2, 1.0);
            let bar = D2D1_ROUNDED_RECT { rect: rect(x, cy - half, x + 3.0, cy + half), radiusX: 1.5, radiusY: 1.5 };
            self.target.FillRoundedRectangle(&bar, &accent);
        }
        Ok(())
    }

    /// Kolom tetap di kiri/kanan halaman: logo dan panah halaman.
    unsafe fn paint_deck(&self, v: &View, alpha: f32, base: Matrix3x2) -> Result<()> {
        let theme = v.theme;
        self.fill_icon_box(&self.icons.logo, ARROW_LEFT_X - 8.0, 14.0, 16.0, 100.0, &self.brush(LOGO_COLOR, alpha)?, base);
        let ink = self.brush(theme.ink, alpha)?;
        for (x, icon, hit) in [(ARROW_LEFT_X, &self.icons.arrow_left, Hit::PageLeft), (ARROW_RIGHT_X, &self.icons.arrow_right, Hit::PageRight)] {
            self.button_circle(x, ARROW_Y, 12.0, theme, v.hover == hit, alpha)?;
            self.stroke_icon(icon, x, ARROW_Y, 12.0, 1.8, &ink, base);
        }
        Ok(())
    }

    unsafe fn paint_music(&mut self, v: &View, alpha: f32, elapsed: f64, duration: f64, fraction: f32, base: Matrix3x2) -> Result<()> {
        let rt = self.target.clone();
        let theme = v.theme;
        let media = v.media;
        let white = self.brush(TEXT, alpha)?;
        let ink = self.brush(theme.ink, alpha)?;
        let accent = self.brush(theme.accent, alpha)?;
        let enabled = media.active;

        // Cover 48 x 48, sudut 10, cadangan ikon not musik.
        let cover = rect(PAGE_LEFT, 10.0, PAGE_LEFT + 48.0, 58.0);
        let cover_shape = D2D1_ROUNDED_RECT { rect: cover, radiusX: 10.0, radiusY: 10.0 };
        rt.FillRoundedRectangle(&cover_shape, &self.brush(theme.surface, alpha)?);
        match &self.cover {
            Some((_, bitmap)) if media.cover.is_some() => {
                let size = bitmap.GetSize();
                let scale = (48.0 / size.width).max(48.0 / size.height);
                let brush = rt.CreateBitmapBrush(bitmap, None, None)?;
                brush.SetTransform(&Matrix3x2 {
                    M11: scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: scale,
                    M31: cover.left + (48.0 - size.width * scale) / 2.0,
                    M32: cover.top + (48.0 - size.height * scale) / 2.0,
                });
                brush.SetOpacity(alpha);
                rt.FillRoundedRectangle(&cover_shape, &brush);
            }
            _ => {
                let (cx, cy) = (PAGE_LEFT + 24.0, 34.0);
                self.stroke_icon(&self.icons.note, cx, cy, 24.0, 1.6, &accent, base);
                let s = 24.0 / 20.0;
                for (ex, ey) in [(5.5, 15.0), (14.5, 13.0)] {
                    rt.FillEllipse(&ellipse2(cx - 12.0 + ex * s, cy - 12.0 + ey * s, 2.5 * s, 2.0 * s), &accent);
                }
            }
        }

        // Jam + tanggal, judul, artis.
        let (text_left, text_right) = (PAGE_LEFT + 58.0, PLAYER_RIGHT - 32.0);
        let clock_width = self.measure(v.clock, &self.formats.small_bold);
        self.text(v.clock, &self.formats.small_bold, rect(text_left, 9.0, text_left + clock_width + 2.0, 22.0), &white);
        self.text(v.date, &self.formats.small, rect(text_left + clock_width + 6.0, 9.0, text_right, 22.0), &white);
        let (title, artist) = if media.active {
            (if media.title.is_empty() { "Media sedang aktif" } else { media.title.as_str() }, media.artist.as_str())
        } else {
            ("Belum ada musik", "Putar lagu di pemutar favoritmu")
        };
        self.text(title, &self.formats.title, rect(text_left, 24.0, text_right, 43.0), &white);
        self.text(artist, &self.formats.artist, rect(text_left, 44.5, text_right, 59.5), &white);

        // Waktu dan slider satu baris.
        let has_time = enabled && duration > 0.0;
        let (elapsed_text, duration_text) = if has_time {
            (clock_text(elapsed), clock_text(duration))
        } else {
            ("--:--".to_string(), "--:--".to_string())
        };
        let elapsed_width = self.measure(&elapsed_text, &self.formats.time_left);
        let duration_width = self.measure(&duration_text, &self.formats.time_right);
        self.track = seek_track(elapsed_width, duration_width);
        let (left, right) = self.track;
        self.text(&elapsed_text, &self.formats.time_left, rect(PAGE_LEFT, 66.0, left, 88.0), &white);
        self.text(&duration_text, &self.formats.time_right, rect(right, 66.0, PLAYER_RIGHT, 88.0), &white);
        let slider_alpha = if has_time { alpha } else { alpha * 0.55 };
        let knob = left + 8.0 + (right - left - 16.0) * if has_time { fraction } else { 0.0 };
        let bar = |l: f32, r: f32| D2D1_ROUNDED_RECT { rect: rect(l, ROW_SEEK_Y - 1.5, r, ROW_SEEK_Y + 1.5), radiusX: 1.5, radiusY: 1.5 };
        rt.FillRoundedRectangle(&bar(knob, right), &self.brush(theme.soft, slider_alpha * 0.5)?);
        rt.FillRoundedRectangle(&bar(left, knob), &self.brush(theme.accent, slider_alpha)?);
        let knob_radius = if matches!(v.hover, Hit::Seek(_)) { 7.0 } else { 5.0 };
        rt.FillEllipse(&ellipse(knob, ROW_SEEK_Y, knob_radius), &self.brush(theme.accent, slider_alpha)?);

        // Tombol kecil kiri: pin, tema, minimalkan.
        let y = ROW_BUTTONS_Y;
        self.button_circle(PIN_X, y, 11.0, theme, v.hover == Hit::Pin, alpha)?;
        self.stroke_icon(&self.icons.pin, PIN_X, y, 14.0, 1.6, &ink, base);
        if !v.pinned {
            self.stroke_icon(&self.icons.pin_slash, PIN_X, y, 14.0, 1.6, &ink, base);
        }
        self.button_circle(THEME_X, y, 11.0, theme, v.hover == Hit::Theme, alpha)?;
        let s = 14.0 / 20.0;
        let ring = D2D1_ELLIPSE { point: pt(THEME_X, y), radiusX: 8.0 * s, radiusY: 8.0 * s };
        rt.DrawEllipse(&ring, &ink, 1.6 * s, None::<&ID2D1StrokeStyle>);
        self.fill_icon_box(&self.icons.theme_half, THEME_X - 7.0, y - 7.0, 14.0, 20.0, &ink, base);
        self.button_circle(MINIMIZE_X, y, 11.0, theme, v.hover == Hit::Minimize, alpha)?;
        self.stroke_icon(&self.icons.minimize, MINIMIZE_X, y, 14.0, 1.8, &ink, base);

        // Kontrol musik di tengah; nonaktif (40%) bila belum ada sesi media.
        let control_alpha = if enabled { alpha } else { alpha * 0.4 };
        let control_ink = self.brush(theme.ink, control_alpha)?;
        self.button_circle(PREVIOUS_X, y, 14.0, theme, enabled && v.hover == Hit::Previous, control_alpha)?;
        self.fill_icon_box(&self.icons.previous, PREVIOUS_X - 9.0, y - 9.0, 18.0, 20.0, &control_ink, base);
        self.button_circle(NEXT_X, y, 14.0, theme, enabled && v.hover == Hit::Next, control_alpha)?;
        self.fill_icon_box(&self.icons.next, NEXT_X - 9.0, y - 9.0, 18.0, 20.0, &control_ink, base);

        // Cincin progres di sekeliling tombol putar: mulai dari atas, searah jarum jam.
        let ring = D2D1_ELLIPSE { point: pt(CONTROLS_X, y), radiusX: 20.0, radiusY: 20.0 };
        rt.DrawEllipse(&ring, &self.brush(theme.soft, alpha * 0.45)?, 2.0, None::<&ID2D1StrokeStyle>);
        if has_time && fraction > 0.0 {
            if fraction >= 0.999 {
                rt.DrawEllipse(&ring, &accent, 2.0, &self.round);
            } else {
                let angle = fraction * std::f32::consts::TAU;
                let end = pt(CONTROLS_X + 20.0 * angle.sin(), y - 20.0 * angle.cos());
                let arc = self.factory.CreatePathGeometry()?;
                let sink = arc.Open()?;
                sink.BeginFigure(pt(CONTROLS_X, y - 20.0), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: end,
                    size: D2D_SIZE_F { width: 20.0, height: 20.0 },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.Close()?;
                rt.DrawGeometry(&arc, &accent, 2.0, &self.round);
            }
        }
        let play_hover = enabled && v.hover == Hit::PlayPause;
        rt.FillEllipse(&ellipse(CONTROLS_X, y, 17.0), &self.brush(theme.accent, if play_hover { control_alpha * 0.85 } else { control_alpha })?);
        let icon = if media.playing { &self.icons.pause } else { &self.icons.play };
        self.fill_icon_box(icon, CONTROLS_X - 9.0, y - 9.0, 18.0, 20.0, &control_ink, base);

        // Tombol jendela lirik (Aa): cari dan pilih lirik sendiri.
        self.button_circle(LYRICS_BUTTON_X, y, 12.0, theme, enabled && v.hover == Hit::Lyrics, control_alpha)?;
        self.stroke_icon(&self.icons.lyrics, LYRICS_BUTTON_X, y, 16.0, 1.6, &control_ink, base);

        // Pemisah dan kolom lirik: baris sebelumnya, baris sekarang (kotak), baris berikutnya.
        rt.FillRectangle(&rect(364.0, 24.0, 365.0, 134.0), &self.brush(theme.soft, alpha * 0.35)?);
        let side = self.brush(TEXT, alpha * 0.45)?;
        self.text_fit(v.lyric.previous, &self.formats.lyric_side, rect(LYRICS_LEFT, 24.0, LYRICS_RIGHT, 44.0), &side, base)?;
        let box_rect = rect(LYRICS_LEFT, 46.0, LYRICS_RIGHT, 112.0);
        rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT { rect: box_rect, radiusX: 7.0, radiusY: 7.0 },
            &self.brush(LYRIC_BOX.0, LYRIC_BOX.1 * alpha)?,
        );
        self.text_fit(v.lyric.current, &self.formats.lyric, rect(LYRICS_LEFT + 4.0, 48.0, LYRICS_RIGHT - 4.0, 110.0), &accent, base)?;
        self.text_fit(v.lyric.next, &self.formats.lyric_side, rect(LYRICS_LEFT, 114.0, LYRICS_RIGHT, 134.0), &side, base)?;
        Ok(())
    }

    /// Halaman clipboard: judul + Jeda/Reset, kotak cari, daftar salinan, dan status.
    unsafe fn paint_clipboard(&self, v: &View, alpha: f32, base: Matrix3x2) -> Result<()> {
        let rt = &self.target;
        let theme = v.theme;
        let clip = &v.clip;
        let white = self.brush(TEXT, alpha)?;
        let ink = self.brush(theme.ink, alpha)?;

        // Judul dan jumlah.
        let title_width = self.measure("Riwayat", &self.formats.clip_title);
        self.text("Riwayat", &self.formats.clip_title, rect(PAGE_LEFT, 8.0, PAGE_LEFT + title_width + 2.0, 30.0), &white);
        let count = format!("{} / {}", clip.total, crate::clipboard::MAX_ITEMS);
        self.text(&count, &self.formats.clip_count, rect(PAGE_LEFT + title_width + 8.0, 8.0, PAGE_LEFT + title_width + 80.0, 30.0), &self.brush(TEXT, alpha * 0.75)?);

        // Tombol pil Jeda/Lanjut dan Reset.
        let pause_label = if clip.recording { "Jeda" } else { "Lanjut" };
        for ((left, right), label, hit, enabled) in [
            (CLIP_PAUSE, pause_label, Hit::ClipPause, true),
            (CLIP_RESET, "Reset", Hit::ClipReset, clip.total > 0),
        ] {
            let chip_alpha = if enabled { alpha } else { alpha * 0.4 };
            let hover = enabled && v.hover == hit;
            let pill = D2D1_ROUNDED_RECT { rect: rect(left, CLIP_HEADER_Y - 10.0, right, CLIP_HEADER_Y + 10.0), radiusX: 10.0, radiusY: 10.0 };
            rt.FillRoundedRectangle(&pill, &self.brush(if hover { theme.accent } else { theme.surface }, chip_alpha)?);
            let fg = if hover { theme.ink } else { TEXT };
            self.text(label, &self.formats.chip, pill.rect, &self.brush(fg, chip_alpha)?);
        }

        // Kotak cari.
        let search = D2D1_ROUNDED_RECT { rect: rect(PAGE_LEFT, CLIP_SEARCH_TOP, PAGE_RIGHT, CLIP_SEARCH_BOTTOM), radiusX: 7.0, radiusY: 7.0 };
        rt.FillRoundedRectangle(&search, &self.brush(theme.soft, alpha)?);
        let text_area = rect(PAGE_LEFT + 9.0, CLIP_SEARCH_TOP, PAGE_RIGHT - 9.0, CLIP_SEARCH_BOTTOM);
        if clip.search.is_empty() {
            self.text("Cari kata dalam clipboard…", &self.formats.search, text_area, &self.brush(theme.ink, alpha * 0.5)?);
        } else {
            self.text(clip.search, &self.formats.search, text_area, &ink);
        }
        if clip.focused && clip.caret {
            let x = (text_area.left + self.measure(clip.search, &self.formats.search) + 1.0).min(text_area.right);
            let middle = (CLIP_SEARCH_TOP + CLIP_SEARCH_BOTTOM) / 2.0;
            rt.FillRectangle(&rect(x, middle - 7.0, x + 1.0, middle + 7.0), &ink);
        }

        // Daftar salinan (bisa digulir).
        let right = clip_list_right(clip.rows.len());
        let (time_x, copy_x, delete_x) = clip_columns(right);
        rt.PushAxisAlignedClip(&rect(PAGE_LEFT, CLIP_LIST_TOP, PAGE_RIGHT, CLIP_LIST_BOTTOM), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let first = (clip.scroll / CLIP_ROW_H) as usize;
        let row_hover = self.brush(LYRIC_BOX.0, alpha * 0x38 as f32 / 255.0)?;
        let muted = self.brush(TEXT, alpha * 0.65)?;
        for (index, row) in clip.rows.iter().enumerate().skip(first).take(4) {
            let top = CLIP_LIST_TOP - clip.scroll + index as f32 * CLIP_ROW_H;
            let cy = top + CLIP_ROW_H / 2.0;
            let hovered = matches!(v.hover, Hit::ClipRow(id) | Hit::ClipCopy(id) | Hit::ClipDelete(id) if id == row.id);
            if hovered {
                let bg = D2D1_ROUNDED_RECT { rect: rect(PAGE_LEFT, top + 1.0, right, top + CLIP_ROW_H - 1.0), radiusX: 5.0, radiusY: 5.0 };
                rt.FillRoundedRectangle(&bg, &row_hover);
            }
            self.text(row.preview, &self.formats.row, rect(PAGE_LEFT + 5.0, top, time_x - 19.0, top + CLIP_ROW_H), &white);
            self.text(row.time, &self.formats.row_time, rect(time_x - 19.0, top, time_x + 19.0, top + CLIP_ROW_H), &muted);
            self.button_circle(copy_x, cy, 11.0, theme, v.hover == Hit::ClipCopy(row.id), alpha)?;
            self.stroke_icon(&self.icons.copy, copy_x, cy, 13.0, 1.5, &ink, base);
            self.button_circle(delete_x, cy, 9.0, theme, v.hover == Hit::ClipDelete(row.id), alpha)?;
            self.stroke_icon(&self.icons.close, delete_x, cy, 11.0, 1.7, &ink, base);
        }
        rt.PopAxisAlignedClip();
        let content = clip.rows.len() as f32 * CLIP_ROW_H;
        if content > CLIP_LIST_H {
            // Scrollbar: jalur tipis + pegangan Soft.
            let track = rect(PAGE_RIGHT - 10.0, CLIP_LIST_TOP + 1.0, PAGE_RIGHT - 2.0, CLIP_LIST_BOTTOM - 1.0);
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: 4.0, radiusY: 4.0 }, &self.brush(LYRIC_BOX.0, alpha * LYRIC_BOX.1)?);
            let span = track.bottom - track.top;
            let thumb_h = (span * CLIP_LIST_H / content).max(12.0);
            let thumb_top = track.top + (span - thumb_h) * (clip.scroll / (content - CLIP_LIST_H)).clamp(0.0, 1.0);
            let thumb = rect(track.left + 1.0, thumb_top, track.right - 1.0, thumb_top + thumb_h);
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: thumb, radiusX: 3.0, radiusY: 3.0 }, &self.brush(theme.soft, alpha * 0.8)?);
        }
        if clip.rows.is_empty() {
            let empty = if clip.total == 0 { "Salin teks dengan Ctrl+C untuk memulai." } else { "Tidak ada salinan yang cocok dengan pencarian." };
            self.text(empty, &self.formats.note_center, rect(PAGE_LEFT, CLIP_LIST_TOP, PAGE_RIGHT, CLIP_LIST_BOTTOM), &self.brush(TEXT, alpha * 0.7)?);
        }
        self.text(clip.status, &self.formats.status, rect(PAGE_LEFT + 12.0, CLIP_LIST_BOTTOM, PAGE_RIGHT - 12.0, 142.0), &self.brush(TEXT, alpha * 0.8)?);
        Ok(())
    }

    /// Halaman salat: kartu waktu berikutnya + lintasan matahari (busur siang, matahari/bulan saat ini,
    /// label sejajar di bawah cakrawala seperti sumbu waktu).
    unsafe fn paint_prayer(&self, v: &View, alpha: f32, base: Matrix3x2) -> Result<()> {
        let rt = &self.target;
        let theme = v.theme;
        let p = &v.prayer;
        let white = self.brush(TEXT, alpha)?;
        let accent = self.brush(theme.accent, alpha)?;
        let dim = self.brush(TEXT, alpha * 0.33)?;
        let sun = self.brush(SUN, alpha)?;

        self.text("Jadwal Salat", &self.formats.clip_title, rect(PAGE_LEFT, 8.0, PAGE_LEFT + 200.0, 30.0), &white);
        // Pilihan kota: pil seperti tombol tema, klik membuka daftar kota.
        let (l, r) = PRAYER_CITY;
        let pill = D2D1_ROUNDED_RECT { rect: rect(l, PRAYER_HEADER_Y - 11.0, r, PRAYER_HEADER_Y + 11.0), radiusX: 7.0, radiusY: 7.0 };
        let hover = v.hover == Hit::PrayerCity;
        rt.FillRoundedRectangle(&pill, &self.brush(if hover { theme.accent } else { theme.soft }, alpha)?);
        let ink = self.brush(theme.ink, alpha)?;
        self.text(p.city, &self.formats.search, rect(l + 10.0, pill.rect.top, r - 24.0, pill.rect.bottom), &ink);
        self.stroke_icon(&self.icons.chevron, r - 13.0, PRAYER_HEADER_Y, 12.0, 1.8, &ink, base);

        // Kartu waktu berikutnya.
        let (cl, ct, cr, cb) = PRAYER_CARD;
        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(cl, ct, cr, cb), radiusX: 12.0, radiusY: 12.0 }, &accent);
        let label = if p.next.is_none() { "BESOK" } else { "BERIKUTNYA" };
        self.text(label, &self.formats.card_label, rect(cl + 12.0, ct + 12.0, cr - 8.0, ct + 26.0), &self.brush(theme.ink, alpha * 0.7)?);
        // Nama + jam satu baris; bila nama panjang (Maghrib) keduanya diperkecil bersama, tidak dipotong "…".
        let time = p.next_time.label();
        let name_w = self.measure(p.next_time.name, &self.formats.card_name);
        let time_w = self.measure(&time, &self.formats.card_time);
        let (left, gap) = (cl + 12.0, 5.0);
        let scale = ((cr - 10.0 - left) / (name_w + gap + time_w)).min(1.0);
        let row = Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: left * (1.0 - scale), M32: (ct + 42.0) * (1.0 - scale) };
        rt.SetTransform(&multiply(&row, &base));
        self.text(p.next_time.name, &self.formats.card_name, rect(left, ct + 28.0, left + name_w + 2.0, ct + 56.0), &ink);
        self.text(&time, &self.formats.card_time, rect(left + name_w + gap, ct + 26.0, left + name_w + gap + time_w + 2.0, ct + 56.0), &ink);
        rt.SetTransform(&base);
        self.text(&format!("{} · {}", p.countdown, p.zone), &self.formats.card_note, rect(cl + 12.0, ct + 58.0, cr - 6.0, ct + 76.0), &ink);

        // Lintasan matahari.
        let sky = crate::prayer::Sky { times: &p.times, width: SKY_W, height: SKY_H };
        let at = |t: f64| (SKY_LEFT + sky.x(t), SKY_TOP + sky.y(t));
        let horizon = SKY_TOP + sky.horizon();
        rt.DrawLine(pt(SKY_LEFT, horizon), pt(SKY_LEFT + SKY_W, horizon), &dim, 1.0, None::<&ID2D1StrokeStyle>);
        let (rise, set) = (p.times[1].minutes, p.times[4].minutes);
        rt.DrawGeometry(&self.sky_arc(&at, rise, set)?, &dim, 1.5, &self.dash);
        if p.now > rise {
            rt.DrawGeometry(&self.sky_arc(&at, rise, p.now.min(set))?, &accent, 2.0, &self.round);
        }
        for (i, time) in p.times.iter().enumerate() {
            let (x, y) = at(time.minutes);
            let is_next = p.next == Some(i);
            let past = time.minutes <= p.now;
            let fade = if past && !is_next { 0.5 } else { 1.0 };
            if y < horizon - 1.0 {
                rt.DrawLine(pt(x, y + 5.0), pt(x, horizon), &dim, 1.0, &self.dot);
            }
            if i == 1 || i == 4 {
                // Terbit/Maghrib: matahari setengah di cakrawala dengan tiga sinar.
                rt.FillGeometry(&self.half_sun(x, horizon)?, &sun, None::<&ID2D1Brush>);
                for (dx, dy, ex, ey) in [(0.0, -9.5, 0.0, -12.5), (-7.4, -6.8, -9.5, -8.9), (7.4, -6.8, 9.5, -8.9)] {
                    rt.DrawLine(pt(x + dx, horizon + dy), pt(x + ex, horizon + ey), &sun, 1.4, &self.round);
                }
            }
            if !(i == 1 || i == 4) || is_next {
                let r = if is_next { 4.5 } else { 3.0 };
                let fill = if past { theme.accent } else { 0x000000 };
                rt.FillEllipse(&ellipse(x, y, r), &self.brush(fill, alpha * fade)?);
                let ring = if is_next || past { theme.accent } else { TEXT };
                rt.DrawEllipse(&ellipse(x, y, r), &self.brush(ring, alpha * fade)?, if is_next { 2.0 } else { 1.2 }, None::<&ID2D1StrokeStyle>);
            }
            let color = if is_next { theme.accent } else { TEXT };
            self.text(time.name, &self.formats.sky_name, rect(x - 26.0, horizon + 4.0, x + 26.0, horizon + 15.0), &self.brush(color, alpha * fade * if is_next { 1.0 } else { 0.8 })?);
            self.text(&time.label(), &self.formats.sky_time, rect(x - 26.0, horizon + 14.0, x + 26.0, horizon + 30.0), &self.brush(color, alpha * fade)?);
        }
        // Siang: matahari bercahaya di busur. Malam: bulan sabit di atas cakrawala.
        let (sx, sy) = at(p.now);
        if sky.is_day(p.now) {
            rt.FillEllipse(&ellipse(sx, sy, 10.0), &self.brush(SUN, alpha * 0.18)?);
            rt.FillEllipse(&ellipse(sx, sy, 7.5), &self.brush(SUN, alpha * 0.3)?);
            rt.FillEllipse(&ellipse(sx, sy, 5.5), &sun);
        } else {
            // Sabit berat ke kiri: geser +2 agar tampak tepat di atas posisinya, dan beri jarak dari titik cakrawala.
            rt.FillGeometry(&self.crescent(sx + 2.0, horizon - 17.0)?, &self.brush(0xE8ECF4, alpha)?, None::<&ID2D1Brush>);
        }
        Ok(())
    }

    unsafe fn sky_arc(&self, at: &dyn Fn(f64) -> (f32, f32), from: f64, to: f64) -> Result<ID2D1PathGeometry> {
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let (x, y) = at(from);
        sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_HOLLOW);
        for i in 1..=48 {
            let (x, y) = at(from + (to - from) * i as f64 / 48.0);
            sink.AddLine(pt(x, y));
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        Ok(geometry)
    }

    unsafe fn half_sun(&self, x: f32, y: f32) -> Result<ID2D1PathGeometry> {
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(x - 7.0, y), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: pt(x + 7.0, y),
            size: D2D_SIZE_F { width: 7.0, height: 7.0 },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        });
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        Ok(geometry)
    }

    /// Bulan sabit: lingkaran dikurangi lingkaran yang digeser ke kanan atas.
    unsafe fn crescent(&self, cx: f32, cy: f32) -> Result<ID2D1PathGeometry> {
        let outer = self.factory.CreateEllipseGeometry(&ellipse(cx, cy, 6.0))?;
        let inner = self.factory.CreateEllipseGeometry(&ellipse(cx + 3.0, cy - 2.0, 5.5))?;
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        outer.CombineWithGeometry(&inner, D2D1_COMBINE_MODE_EXCLUDE, None, 0.05, &sink)?;
        sink.Close()?;
        Ok(geometry)
    }

    /// Teks di tengah area; diperkecil seragam bila tidak muat (seperti Viewbox StretchDirection=DownOnly).
    unsafe fn text_fit(&self, s: &str, format: &IDWriteTextFormat, area: D2D_RECT_F, brush: &ID2D1SolidColorBrush, base: Matrix3x2) -> Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let (aw, ah) = (area.right - area.left, area.bottom - area.top);
        let wide: Vec<u16> = s.encode_utf16().collect();
        let wraps = format.GetWordWrapping() != DWRITE_WORD_WRAPPING_NO_WRAP;
        let layout = self.dwrite.CreateTextLayout(&wide, format, if wraps { aw } else { 10_000.0 }, 10_000.0)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics)?;
        let scale = (aw / metrics.widthIncludingTrailingWhitespace.max(1.0)).min(ah / metrics.height.max(1.0)).min(1.0);
        if scale >= 0.999 {
            self.text(s, format, area, brush);
            return Ok(());
        }
        let (cx, cy) = ((area.left + area.right) / 2.0, (area.top + area.bottom) / 2.0);
        let local = Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: cx * (1.0 - scale), M32: cy * (1.0 - scale) };
        self.target.SetTransform(&multiply(&local, &base));
        let grown = rect(cx - aw / scale / 2.0, cy - ah / scale / 2.0, cx + aw / scale / 2.0, cy + ah / scale / 2.0);
        self.text(s, format, grown, brush);
        self.target.SetTransform(&base);
        Ok(())
    }

    /// Lebar teks lirik notch kecil, untuk menghitung lebar notch.
    pub fn compact_lyric_width(&self, line: &str) -> f32 {
        unsafe { self.measure(line, &self.formats.compact_lyric) }
    }

    /// Tombol bulat gaya MusicButton: latar Soft, Accent saat di-hover.
    unsafe fn button_circle(&self, x: f32, y: f32, r: f32, theme: &Theme, hover: bool, alpha: f32) -> Result<()> {
        let fill = if hover { theme.accent } else { theme.soft };
        self.target.FillEllipse(&ellipse(x, y, r), &self.brush(fill, alpha)?);
        Ok(())
    }

    /// Isi geometri berbidang `box_size` ke kotak `size` di (left, top).
    unsafe fn fill_icon_box(&self, geometry: &impl Interface, left: f32, top: f32, size: f32, box_size: f32, brush: &ID2D1SolidColorBrush, base: Matrix3x2) {
        let s = size / box_size;
        let local = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: left, M32: top };
        self.target.SetTransform(&multiply(&local, &base));
        let geometry: ID2D1Geometry = geometry.cast().expect("geometry");
        self.target.FillGeometry(&geometry, brush, None::<&ID2D1Brush>);
        self.target.SetTransform(&base);
    }

    /// Garis ikon 20 x 20 dengan ujung membulat, dipusatkan di (cx, cy) selebar `size`.
    unsafe fn stroke_icon(&self, geometry: &ID2D1PathGeometry, cx: f32, cy: f32, size: f32, width: f32, brush: &ID2D1SolidColorBrush, base: Matrix3x2) {
        let s = size / 20.0;
        let local = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx - size / 2.0, M32: cy - size / 2.0 };
        self.target.SetTransform(&multiply(&local, &base));
        self.target.DrawGeometry(geometry, brush, width, &self.round);
        self.target.SetTransform(&base);
    }

    unsafe fn measure(&self, s: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let Ok(layout) = self.dwrite.CreateTextLayout(&wide, format, 1000.0, 100.0) else { return 0.0 };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut metrics).is_err() {
            return 0.0;
        }
        metrics.widthIncludingTrailingWhitespace
    }

    /// Cover baru didekode sekali per lagu, diperkecil ke maksimal 256 px agar RAM tetap kecil.
    unsafe fn update_cover(&mut self, media: &MediaState) {
        let Some(bytes) = &media.cover else {
            self.cover = None;
            return;
        };
        if matches!(&self.cover, Some((id, _)) if *id == media.cover_id) {
            return;
        }
        self.cover = self.decode(bytes).ok().map(|bitmap| (media.cover_id, bitmap));
    }

    unsafe fn decode(&self, bytes: &[u8]) -> Result<ID2D1Bitmap> {
        let stream = self.wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = self.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height)?;
        let scale = (256.0 / width.max(height).max(1) as f32).min(1.0);
        let scaler = self.wic.CreateBitmapScaler()?;
        scaler.Initialize(
            &frame,
            ((width as f32 * scale) as u32).max(1),
            ((height as f32 * scale) as u32).max(1),
            WICBitmapInterpolationModeFant,
        )?;
        let converter = self.wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeMedianCut,
        )?;
        self.target.CreateBitmapFromWicBitmap(&converter, None)
    }

    unsafe fn brush(&self, rgb: u32, alpha: f32) -> Result<ID2D1SolidColorBrush> {
        self.target.CreateSolidColorBrush(&color(rgb, alpha), None)
    }

    unsafe fn text(&self, s: &str, format: &IDWriteTextFormat, area: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        self.target.DrawText(&wide, format, &area, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
}

/// Direct3D11 (GPU; WARP bila tidak ada GPU) -> Direct2D DeviceContext + DirectComposition.
unsafe fn create_gpu(factory: &ID2D1Factory) -> Result<Gpu> {
    let create = |driver: D3D_DRIVER_TYPE| -> Result<ID3D11Device> {
        let mut device = None;
        D3D11CreateDevice(None::<&IDXGIAdapter>, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None,
            D3D11_SDK_VERSION, Some(&mut device), None, None)?;
        Ok(device.unwrap())
    };
    let d3d = create(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create(D3D_DRIVER_TYPE_WARP))?;
    let dxgi: IDXGIDevice = d3d.cast()?;
    // Antrean satu frame: animasi mengikuti vsync tanpa tertinggal beberapa frame.
    let _ = dxgi.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1);
    let device = factory.cast::<ID2D1Factory1>()?.CreateDevice(&dxgi)?;
    let context = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
    let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
    Ok(Gpu { d3d, dxgi_factory, dcomp, context, swap: None, composition: None })
}
unsafe fn format(
    dwrite: &IDWriteFactory,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    align: DWRITE_TEXT_ALIGNMENT,
    ellipsis: bool,
    wrap: bool,
) -> Result<IDWriteTextFormat> {
    let format = dwrite.CreateTextFormat(
        w!("Segoe UI"),
        None,
        weight,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        size,
        w!("id-ID"),
    )?;
    format.SetTextAlignment(align)?;
    format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    format.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
    if ellipsis {
        let sign = dwrite.CreateEllipsisTrimmingSign(&format)?;
        let trimming = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        format.SetTrimming(&trimming, &sign)?;
    }
    Ok(format)
}

/// Logo N NotchMusic (bidang 100 x 100): bentuk N dikurangi dua celah diagonal,
/// sama dengan CombinedGeometry NotchLogoGeometry di XAML.
unsafe fn logo_geometry(factory: &ID2D1Factory) -> Result<ID2D1Geometry> {
    let n = path_with_mode(factory, "M0 8 L8 0 H25 L75 59 V8 L83 0 H100 V92 L92 100 H75 L25 41 V92 L17 100 H0 Z", true, false)?;
    let slot = |cap: &str, tx: f32, ty: f32| -> Result<ID2D1Geometry> {
        let bar = path(factory, "M0,0 H57 V6.5 H0 Z", true)?;
        let end = path(factory, cap, true)?;
        let group = factory.CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(bar.cast()?), Some(end.cast()?)])?;
        let mut m = rotation(-42.0, 28.5, 3.25);
        m.M31 += tx;
        m.M32 += ty;
        Ok(factory.CreateTransformedGeometry(&group, &m)?.cast()?)
    };
    let upper = slot("M19.38 4.55 H37.62 V12.948 A5.8368 3.952 0 0 1 31.7832 16.9 H25.2168 A5.8368 3.952 0 0 1 19.38 12.948 Z", 0.0, 21.0)?;
    let lower = slot("M19.38 1.95 V-6.448 A5.8368 3.952 0 0 1 25.2168 -10.4 H31.7832 A5.8368 3.952 0 0 1 37.62 -6.448 V1.95 Z", 43.0, 72.5)?;
    let cuts = factory.CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(upper), Some(lower)])?;
    let result = factory.CreatePathGeometry()?;
    let sink = result.Open()?;
    n.CombineWithGeometry(&cuts, D2D1_COMBINE_MODE_EXCLUDE, None, 0.05, &sink)?;
    sink.Close()?;
    result.cast()
}

unsafe fn path(factory: &ID2D1Factory, data: &str, filled: bool) -> Result<ID2D1PathGeometry> {
    path_with_mode(factory, data, filled, true)
}

/// Pengurai data path SVG/XAML kecil: perintah absolut M, L, H, V, C, A, Z.
unsafe fn path_with_mode(factory: &ID2D1Factory, data: &str, filled: bool, even_odd: bool) -> Result<ID2D1PathGeometry> {
    let geometry = factory.CreatePathGeometry()?;
    let sink = geometry.Open()?;
    sink.SetFillMode(if even_odd { D2D1_FILL_MODE_ALTERNATE } else { D2D1_FILL_MODE_WINDING });
    let begin: D2D1_FIGURE_BEGIN = if filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
    let tokens = tokenize(data);
    let (mut i, mut command) = (0usize, 'M');
    let (mut cur, mut start, mut open) = ((0f32, 0f32), (0f32, 0f32), false);
    let num = |i: &mut usize| -> f32 {
        let value = match tokens.get(*i) {
            Some(Token::Number(v)) => *v,
            _ => 0.0,
        };
        *i += 1;
        value
    };
    while i < tokens.len() {
        if let Token::Command(c) = tokens[i] {
            command = c;
            i += 1;
            if c == 'Z' {
                if open {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    open = false;
                }
                cur = start;
            }
            continue;
        }
        match command {
            'M' => {
                if open {
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
                cur = (num(&mut i), num(&mut i));
                start = cur;
                sink.BeginFigure(pt(cur.0, cur.1), begin);
                open = true;
                command = 'L'; // Pasangan berikutnya setelah M adalah garis.
            }
            'L' => {
                cur = (num(&mut i), num(&mut i));
                sink.AddLine(pt(cur.0, cur.1));
            }
            'H' => {
                cur.0 = num(&mut i);
                sink.AddLine(pt(cur.0, cur.1));
            }
            'V' => {
                cur.1 = num(&mut i);
                sink.AddLine(pt(cur.0, cur.1));
            }
            'C' => {
                let p1 = (num(&mut i), num(&mut i));
                let p2 = (num(&mut i), num(&mut i));
                cur = (num(&mut i), num(&mut i));
                sink.AddBezier(&bezier(p1, p2, cur));
            }
            'A' => {
                let (rx, ry, rotation) = (num(&mut i), num(&mut i), num(&mut i));
                let (large, sweep) = (num(&mut i) != 0.0, num(&mut i) != 0.0);
                cur = (num(&mut i), num(&mut i));
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: pt(cur.0, cur.1),
                    size: D2D_SIZE_F { width: rx, height: ry },
                    rotationAngle: rotation,
                    sweepDirection: if sweep { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                    arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
            }
            _ => i += 1,
        }
    }
    if open {
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
    }
    sink.Close()?;
    Ok(geometry)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Command(char),
    Number(f32),
}

fn tokenize(data: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut number = String::new();
    let flush = |number: &mut String, tokens: &mut Vec<Token>| {
        if !number.is_empty() {
            tokens.push(Token::Number(number.parse().unwrap_or(0.0)));
            number.clear();
        }
    };
    for c in data.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            flush(&mut number, &mut tokens);
            tokens.push(Token::Command(c.to_ascii_uppercase()));
        } else if c == '-' && !number.is_empty() && !number.ends_with(['e', 'E']) {
            flush(&mut number, &mut tokens);
            number.push(c);
        } else if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' {
            if c == '.' && number.contains('.') {
                flush(&mut number, &mut tokens);
            }
            number.push(c);
        } else {
            flush(&mut number, &mut tokens);
        }
    }
    flush(&mut number, &mut tokens);
    tokens
}

/// Rotasi WPF (derajat, searah jarum jam di layar) di sekitar (cx, cy).
fn rotation(degrees: f32, cx: f32, cy: f32) -> Matrix3x2 {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Matrix3x2 {
        M11: cos,
        M12: sin,
        M21: -sin,
        M22: cos,
        M31: cx - cx * cos + cy * sin,
        M32: cy - cx * sin - cy * cos,
    }
}

/// a lalu b (konvensi vektor baris Direct2D).
fn multiply(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

fn sample_bezier(points: &mut Vec<(f32, f32)>, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) {
    for step in 1..=16 {
        let t = step as f32 / 16.0;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        points.push((
            a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
            a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
        ));
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

pub fn clock_text(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn bezier(p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) -> D2D1_BEZIER_SEGMENT {
    D2D1_BEZIER_SEGMENT { point1: pt(p1.0, p1.1), point2: pt(p2.0, p2.1), point3: pt(p3.0, p3.1) }
}

fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE {
    ellipse2(x, y, r, r)
}

fn ellipse2(x: f32, y: f32, rx: f32, ry: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE { point: pt(x, y), radiusX: rx, radiusY: ry }
}

fn color(rgb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_text_formats_minutes_and_hours() {
        assert_eq!(clock_text(0.0), "0:00");
        assert_eq!(clock_text(83.9), "1:23");
        assert_eq!(clock_text(3725.0), "1:02:05");
    }

    #[test]
    fn hit_test_only_works_when_open() {
        let media = MediaState { active: true, end: 2_000_000_000, ..Default::default() };
        let track = seek_track(30.0, 30.0);
        let clip = ClipHit { ids: &[], total: 0, scroll: 0.0 };
        let hit = |t, x, y| hit_test(t, PAGE_MUSIC, &media, track, &clip, x, y);
        assert_eq!(hit(0.5, CONTROLS_X, ROW_BUTTONS_Y), Hit::None);
        assert_eq!(hit(1.0, CONTROLS_X, ROW_BUTTONS_Y), Hit::PlayPause);
        assert_eq!(hit(1.0, PREVIOUS_X, ROW_BUTTONS_Y), Hit::Previous);
        assert_eq!(hit(1.0, PIN_X, ROW_BUTTONS_Y), Hit::Pin);
        assert_eq!(hit(1.0, track.0 + 8.0, ROW_SEEK_Y), Hit::Seek(0.0));
        assert_eq!(hit(1.0, ARROW_RIGHT_X, ARROW_Y), Hit::PageRight);
    }

    #[test]
    fn clipboard_rows_respect_scroll() {
        let media = MediaState::default();
        let ids = [10, 11, 12, 13];
        let clip = ClipHit { ids: &ids, total: 4, scroll: CLIP_ROW_H };
        let right = clip_list_right(ids.len());
        let (_, copy_x, delete_x) = clip_columns(right);
        let y = CLIP_LIST_TOP + CLIP_ROW_H / 2.0;
        let hit = |x, y| hit_test(1.0, PAGE_CLIPBOARD, &media, (0.0, 0.0), &clip, x, y);
        assert_eq!(hit(copy_x, y), Hit::ClipCopy(11));
        assert_eq!(hit(delete_x, y), Hit::ClipDelete(11));
        assert_eq!(hit(PAGE_LEFT + 20.0, y), Hit::ClipRow(11));
        assert_eq!(hit(PAGE_LEFT + 20.0, (CLIP_SEARCH_TOP + CLIP_SEARCH_BOTTOM) / 2.0), Hit::ClipSearch);
    }

    #[test]
    fn tokenizer_splits_commands_and_signed_numbers() {
        assert_eq!(
            tokenize("M19.38 1.95 V-6.448 A5.8368 3.952 0 0 1 25.2168 -10.4"),
            vec![
                Token::Command('M'), Token::Number(19.38), Token::Number(1.95),
                Token::Command('V'), Token::Number(-6.448),
                Token::Command('A'), Token::Number(5.8368), Token::Number(3.952), Token::Number(0.0),
                Token::Number(0.0), Token::Number(1.0), Token::Number(25.2168), Token::Number(-10.4),
            ]
        );
    }

    #[test]
    fn matrix_multiply_applies_left_first() {
        let scale = Matrix3x2 { M11: 2.0, M12: 0.0, M21: 0.0, M22: 2.0, M31: 0.0, M32: 0.0 };
        let shift = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 5.0, M32: 7.0 };
        let m = multiply(&scale, &shift);
        assert_eq!((m.M11, m.M31, m.M32), (2.0, 5.0, 7.0));
    }
}
