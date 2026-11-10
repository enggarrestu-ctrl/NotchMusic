// Editor tema kustom, port ThemeEditor NotchMusic: enam kotak warna (klik = dialog warna Windows,
// sama dengan "Edit colors" di Paint) dan kode hex. Perubahan langsung dipratinjau di notch;
// Batal/tutup mengembalikan tema sebelumnya, Simpan menjadikannya tema "Kustom" (urutan terakhir).
// Jendelanya sendiri memakai warna yang sedang diedit: latar, kolom hex, tombol, dan bilah judul.

use std::cell::{Cell, RefCell};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, FillRgn, GetSysColorBrush,
    InvalidateRect, RedrawWindow, SelectObject, SetBkColor, SetBkMode, SetTextColor, COLOR_BTNFACE, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HFONT, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, TRANSPARENT,
};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::draw::Theme;

const LABELS: [&str; 6] =
    ["Latar notch", "Cover dan garis tepi", "Tombol", "Aksen (hover, progres, lirik)", "Ikon di tombol", "Garis progres notch"];
const SWATCH_ID: usize = 100;
const EDIT_ID: usize = 200;
const SETTINGS_KEY: PCWSTR = w!("Software\\NotchRust");

pub enum Action {
    /// Warna diubah: tampilkan di notch tanpa menyimpan.
    Preview([u32; 6]),
    Save([u32; 6]),
    /// Ditutup tanpa Simpan: kembali ke tema sebelumnya.
    Cancel,
}

struct State {
    hwnd: HWND,
    colors: [u32; 6],
    swatches: [HWND; 6],
    edits: [HWND; 6],
    brushes: [HBRUSH; 6],
    /// Latar jendela (warna latar notch) dan kolom hex (warna cover/garis tepi).
    back: HBRUSH,
    field: HBRUSH,
    font: HFONT,
    handler: fn(Action),
    saved: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    // Warna kustom dialog warna diingat selama aplikasi berjalan.
    static CUSTOM: Cell<[COLORREF; 16]> = const { Cell::new([COLORREF(0xFFFFFF); 16]) };
}

pub fn to_theme(c: [u32; 6]) -> Theme {
    Theme { name: "Kustom", background: c[0], surface: c[1], soft: c[2], accent: c[3], ink: c[4], progress: c[5] }
}

pub fn colors(t: &Theme) -> [u32; 6] {
    [t.background, t.surface, t.soft, t.accent, t.ink, t.progress]
}

/// "#RRGGBB" atau "RRGGBB" (huruf besar/kecil); selain itu None.
pub fn parse_hex(text: &str) -> Option<u32> {
    let digits = text.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(digits, 16).ok())?
}

fn hex(rgb: u32) -> String {
    format!("#{rgb:06X}")
}

/// 5 atau 6 warna (format CustomColors NotchMusic); warna ke-6 (garis progres) bawaannya = aksen.
fn parse_list<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<[u32; 6]> {
    let list: Vec<u32> = items.into_iter().map(parse_hex).collect::<Option<_>>()?;
    match list.len() {
        5 => Some([list[0], list[1], list[2], list[3], list[4], list[3]]),
        6 => list.try_into().ok(),
        _ => None,
    }
}

/// Tema kustom tersimpan; pertama kali jalan memakai tema kustom NotchMusic bila ada.
pub fn load() -> Option<[u32; 6]> {
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let stored = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, w!("CustomTheme"), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr() as _), Some(&mut size))
    };
    if stored.is_ok() {
        let length = buffer.iter().position(|&c| c == 0).unwrap_or(0);
        return parse_list(String::from_utf16_lossy(&buffer[..length]).split(','));
    }
    let base = std::env::var_os("LOCALAPPDATA")?;
    let text = std::fs::read_to_string(std::path::Path::new(&base).join("NotchMusic").join("ui-settings.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    parse_list(json["CustomColors"].as_array()?.iter().filter_map(|v| v.as_str()))
}

pub fn save(colors: [u32; 6]) {
    let text: Vec<u16> = colors.map(hex).join(",").encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, w!("CustomTheme"), REG_SZ.0, Some(text.as_ptr() as _), (text.len() * 2) as u32);
    }
}

fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

/// Teks putih di warna gelap, hampir hitam di warna terang.
fn text_on(rgb: u32) -> COLORREF {
    let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    if r * 299 + g * 587 + b * 114 > 150_000 { COLORREF(0x111111) } else { COLORREF(0xFFFFFF) }
}

/// Warnai ulang jendela editor dari warna yang sedang diedit (juga bilah judul lewat DWM).
unsafe fn retheme(state: &mut State) {
    let _ = DeleteObject(state.back);
    let _ = DeleteObject(state.field);
    state.back = CreateSolidBrush(colorref(state.colors[0]));
    state.field = CreateSolidBrush(colorref(state.colors[1]));
    let caption = colorref(state.colors[0]);
    let text = text_on(state.colors[0]);
    let dark = (text.0 == 0xFFFFFF) as i32;
    let set = |attribute: i32, value: *const std::ffi::c_void| {
        let _ = DwmSetWindowAttribute(state.hwnd, DWMWINDOWATTRIBUTE(attribute), value, 4);
    };
    set(20, &dark as *const i32 as _); // DWMWA_USE_IMMERSIVE_DARK_MODE
    set(35, &caption as *const COLORREF as _); // DWMWA_CAPTION_COLOR (Windows 11)
    set(36, &text as *const COLORREF as _); // DWMWA_TEXT_COLOR
    let _ = RedrawWindow(state.hwnd, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
}

/// Tombol Simpan (aksen, teks warna ikon) dan Batal (warna cover); ditekan = warna tombol.
unsafe fn draw_button(item: &DRAWITEMSTRUCT) {
    let Some((colors, back, font)) = STATE.with(|s| s.try_borrow().ok()?.as_ref().map(|s| (s.colors, s.back, s.font))) else { return };
    let hdc = item.hDC;
    let r = item.rcItem;
    FillRect(hdc, &r, back);
    let pressed = (item.itemState.0 & ODS_SELECTED.0) != 0;
    let fill = if pressed { colors[2] } else if item.CtlID == IDOK.0 as u32 { colors[3] } else { colors[1] };
    let text = if !pressed && item.CtlID == IDOK.0 as u32 { colorref(colors[4]) } else { text_on(fill) };
    let region = CreateRoundRectRgn(r.left, r.top, r.right + 1, r.bottom + 1, 10, 10);
    let brush = CreateSolidBrush(colorref(fill));
    let _ = FillRgn(hdc, region, brush);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(region);
    let mut label = [0u16; 32];
    let length = GetWindowTextW(item.hwndItem, &mut label) as usize;
    let old = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, text);
    let mut area = r;
    DrawTextW(hdc, &mut label[..length], &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(hdc, old);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Buka editor (atau aktifkan bila sudah terbuka) dengan warna tema yang sedang dipakai.
pub unsafe fn open(instance: HINSTANCE, owner: HWND, colors: [u32; 6], topmost: bool, handler: fn(Action)) {
    if let Some(hwnd) = STATE.with(|s| s.borrow().as_ref().map(|s| s.hwnd)) {
        let _ = SetForegroundWindow(hwnd);
        return;
    }
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
        lpszClassName: w!("NotchRustThemeEditor"),
        ..Default::default()
    };
    RegisterClassExW(&class); // Gagal kedua kali karena sudah terdaftar: tidak masalah.
    let ex = if topmost { WS_EX_DLGMODALFRAME | WS_EX_TOPMOST } else { WS_EX_DLGMODALFRAME };
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let Ok(hwnd) = CreateWindowExW(ex, w!("NotchRustThemeEditor"), w!("Edit tema"), style, 0, 0, 0, 0, owner, None, instance, None) else {
        return;
    };
    // shortcut: ukuran dihitung sekali dari DPI layar saat dibuka, tidak menyesuaikan bila dipindah ke layar lain.
    let dpi = GetDpiForWindow(hwnd);
    let px = |dip: i32| dip * dpi as i32 / 96;
    let mut metrics = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    let _ = SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, metrics.cbSize, Some(&mut metrics as *mut _ as _), 0, dpi);
    let font = CreateFontIndirectW(&metrics.lfMessageFont);
    let control = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32, w: i32, h: i32, id: usize| {
        let text = wide(text);
        let child = CreateWindowExW(
            ex,
            class,
            PCWSTR(text.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            px(x),
            px(y),
            px(w),
            px(h),
            hwnd,
            HMENU(id as _),
            instance,
            None,
        )
        .unwrap_or_default();
        SendMessageW(child, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        child
    };
    control(w!("STATIC"), "Klik kotak warna untuk memilih, atau ketik kode hex.", WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 16, 16, 340, 20, 0);
    let mut swatches = [HWND::default(); 6];
    let mut edits = [HWND::default(); 6];
    for i in 0..6 {
        let y = 48 + i as i32 * 32;
        swatches[i] = control(w!("STATIC"), "", WINDOW_STYLE(0x100) /* SS_NOTIFY */ | WS_BORDER, WINDOW_EX_STYLE(0), 16, y, 36, 24, SWATCH_ID + i);
        control(w!("STATIC"), LABELS[i], WINDOW_STYLE(0x200) /* SS_CENTERIMAGE */, WINDOW_EX_STYLE(0), 62, y, 200, 24, 0);
        edits[i] = control(w!("EDIT"), &hex(colors[i]), WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), WS_EX_CLIENTEDGE, 270, y, 84, 24, EDIT_ID + i);
        SendMessageW(edits[i], 0xC5 /* EM_LIMITTEXT */, WPARAM(7), LPARAM(0));
    }
    let buttons_y = 48 + 6 * 32 + 8;
    // Digambar sendiri (WM_DRAWITEM) agar ikut warna tema; Enter tetap = Simpan lewat IsDialogMessage (IDOK).
    let owner_draw = WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32);
    control(w!("BUTTON"), "Simpan", owner_draw, WINDOW_EX_STYLE(0), 186, buttons_y, 80, 28, IDOK.0 as usize);
    control(w!("BUTTON"), "Batal", owner_draw, WINDOW_EX_STYLE(0), 274, buttons_y, 80, 28, IDCANCEL.0 as usize);
    let mut frame = RECT { left: 0, top: 0, right: px(370), bottom: px(buttons_y + 28 + 16) };
    let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex, dpi);
    let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
    let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    let _ = SetWindowPos(hwnd, None, (sw - w) / 2, (sh - h) / 2, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    let brushes = colors.map(|c| CreateSolidBrush(colorref(c)));
    let mut state = State { hwnd, colors, swatches, edits, brushes, back: HBRUSH::default(), field: HBRUSH::default(), font, handler, saved: false };
    retheme(&mut state);
    STATE.with(|s| *s.borrow_mut() = Some(state));
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
}

/// Tab/Enter/Esc di editor; dipanggil dari message loop utama sebelum TranslateMessage.
pub unsafe fn dialog_message(message: &MSG) -> bool {
    match STATE.with(|s| s.borrow().as_ref().map(|s| s.hwnd)) {
        Some(hwnd) => IsDialogMessageW(hwnd, message).as_bool(),
        None => false,
    }
}

/// Kode hex diketik: terapkan begitu lengkap (pratinjau langsung di notch).
unsafe fn typed(i: usize) {
    let Some(edit) = STATE.with(|s| s.borrow().as_ref().map(|s| s.edits[i])) else { return };
    let mut buffer = [0u16; 16];
    let length = GetWindowTextW(edit, &mut buffer) as usize;
    let Some(rgb) = parse_hex(&String::from_utf16_lossy(&buffer[..length])) else { return }; // Tunggu sampai kode lengkap.
    let update = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let state = s.as_mut()?;
        state.colors[i] = rgb;
        let _ = DeleteObject(state.brushes[i]);
        state.brushes[i] = CreateSolidBrush(colorref(rgb));
        let _ = InvalidateRect(state.swatches[i], None, true);
        retheme(state);
        Some((state.handler, state.colors))
    });
    if let Some((handler, colors)) = update {
        handler(Action::Preview(colors));
    }
}

unsafe fn pick(i: usize) {
    let Some((hwnd, start, edit)) = STATE.with(|s| s.borrow().as_ref().map(|s| (s.hwnd, s.colors[i], s.edits[i]))) else { return };
    let mut custom = CUSTOM.get();
    let mut data = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: colorref(start),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR,
        ..Default::default()
    };
    let chosen = ChooseColorW(&mut data).as_bool();
    CUSTOM.set(custom);
    if chosen {
        // colorref() juga membalik BGR ke RGB. SetWindowText memicu EN_CHANGE, jadi jalur ketik yang menerapkan.
        let _ = SetWindowTextW(edit, PCWSTR(wide(&hex(colorref(data.rgbResult.0).0)).as_ptr()));
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_COMMAND => {
            let (id, code) = (wparam.0 & 0xFFFF, (wparam.0 >> 16) as u32);
            match id {
                _ if (SWATCH_ID..SWATCH_ID + 6).contains(&id) && code == STN_CLICKED => pick(id - SWATCH_ID),
                _ if (EDIT_ID..EDIT_ID + 6).contains(&id) && code == EN_CHANGE => typed(id - EDIT_ID),
                _ if id == IDOK.0 as usize => {
                    let saved = STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        let state = s.as_mut()?;
                        state.saved = true;
                        Some((state.handler, state.colors))
                    });
                    if let Some((handler, colors)) = saved {
                        handler(Action::Save(colors));
                    }
                    let _ = DestroyWindow(hwnd);
                }
                _ if id == IDCANCEL.0 as usize => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let back = STATE.with(|s| s.try_borrow().ok()?.as_ref().map(|s| s.back));
            match back {
                Some(back) => {
                    let mut area = RECT::default();
                    let _ = GetClientRect(hwnd, &mut area);
                    FillRect(HDC(wparam.0 as _), &area, back);
                    LRESULT(1)
                }
                None => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
        // Kotak warna = warnanya; label = teks di atas latar tema; kolom hex = warna cover.
        // try_borrow: pesan ini bisa datang saat STATE sedang dipinjam (retheme).
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let control = HWND(lparam.0 as _);
            let hdc = HDC(wparam.0 as _);
            let brush = STATE.with(|s| {
                let s = s.try_borrow().ok()?;
                let state = s.as_ref()?;
                if let Some(i) = state.swatches.iter().position(|&h| h == control) {
                    return Some(state.brushes[i]);
                }
                let (color, brush) = if message == WM_CTLCOLOREDIT { (state.colors[1], state.field) } else { (state.colors[0], state.back) };
                SetTextColor(hdc, text_on(color));
                SetBkColor(hdc, colorref(color));
                Some(brush)
            });
            match brush {
                Some(brush) => LRESULT(brush.0 as isize),
                None => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
        WM_DRAWITEM => {
            draw_button(&*(lparam.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        WM_DESTROY => {
            if let Some(state) = STATE.with(|s| s.borrow_mut().take()) {
                for brush in state.brushes.into_iter().chain([state.back, state.field]) {
                    let _ = DeleteObject(brush);
                }
                let _ = DeleteObject(state.font);
                if !state.saved {
                    (state.handler)(Action::Cancel); // Pratinjau dibatalkan.
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_and_lists_fill_progress() {
        assert_eq!(parse_hex("#7de2b4"), Some(0x7DE2B4));
        assert_eq!(parse_hex("7DE2B4"), Some(0x7DE2B4));
        assert_eq!(parse_hex("#7DE2B"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
        assert_eq!(hex(0x0A0B0C), "#0A0B0C");
        let five = parse_list(["#000000", "#111111", "#222222", "#333333", "#444444"]).unwrap();
        assert_eq!(five[5], 0x333333);
        assert!(parse_list(["#000000"]).is_none());
        assert_eq!(colors(&to_theme(five)), five);
    }
}
