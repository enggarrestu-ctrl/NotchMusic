// Menu klik kanan berwarna tema (owner-draw), seperti ContextMenu bergaya NotchMusic:
// kotak latar notch, baris disorot warna permukaan, tanda centang ✓, pemisah tipis.

use std::cell::RefCell;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetDC, GetTextExtentPoint32W, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HFONT, TRANSPARENT,
};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_CHECKED, ODS_DISABLED, ODS_SELECTED};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, SetMenuInfo, SystemParametersInfoW, HMENU, MENUINFO, MENU_ITEM_FLAGS, MF_OWNERDRAW,
    MF_DISABLED, MIM_APPLYTOSUBMENUS, MIM_BACKGROUND, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

const ROW_H: i32 = 30;
const SEPARATOR_H: i32 = 9;
const CHECK_W: i32 = 28;
const PADDING: i32 = 14;

/// Warna tema (0xRRGGBB): latar, permukaan (sorot/pemisah), teks.
#[derive(Clone, Copy)]
pub struct Colors {
    pub background: u32,
    pub surface: u32,
    pub text: u32,
}

struct State {
    colors: Colors,
    items: Vec<Vec<u16>>,
    font: HFONT,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

/// Mulai menu baru: simpan warna, buat font menu sistem, dan warnai latar menu.
pub unsafe fn begin(menu: HMENU, colors: Colors) {
    end();
    let mut metrics = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    let _ = SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, metrics.cbSize, Some(&mut metrics as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    let font = CreateFontIndirectW(&metrics.lfMenuFont);
    let info = MENUINFO {
        cbSize: std::mem::size_of::<MENUINFO>() as u32,
        fMask: MIM_BACKGROUND | MIM_APPLYTOSUBMENUS,
        hbrBack: CreateSolidBrush(colorref(colors.background)),
        ..Default::default()
    };
    let _ = SetMenuInfo(menu, &info);
    STATE.with(|s| *s.borrow_mut() = Some(State { colors, items: Vec::new(), font }));
}

/// Lepas font setelah menu ditutup (kuas latar ikut dihapus bersama menu oleh Windows? tidak: dibiarkan kecil).
pub unsafe fn end() {
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().take() {
            let _ = DeleteObject(state.font);
        }
    });
}

/// Tambah baris (atau pemisah bila `flags` berisi MF_SEPARATOR) yang digambar sendiri.
pub unsafe fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let index = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let state = s.as_mut().expect("menu::begin");
        state.items.push(text.encode_utf16().collect());
        state.items.len() - 1
    });
    // Untuk MF_OWNERDRAW, lpNewItem dipakai sebagai data item: indeks teks (+1 agar tidak nol).
    let _ = AppendMenuW(menu, flags | MF_OWNERDRAW, id, PCWSTR((index + 1) as *const u16));
}

pub unsafe fn measure(hwnd: HWND, lparam: LPARAM) -> bool {
    let item = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
    STATE.with(|s| {
        let s = s.borrow();
        let Some(state) = s.as_ref() else { return false };
        let Some(text) = state.items.get(item.itemData.wrapping_sub(1)) else { return false };
        if text.is_empty() {
            item.itemHeight = SEPARATOR_H as u32;
            item.itemWidth = 40;
            return true;
        }
        let dc = GetDC(hwnd);
        let old = SelectObject(dc, state.font);
        let mut size = Default::default();
        let _ = GetTextExtentPoint32W(dc, text, &mut size);
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
        item.itemWidth = (CHECK_W + size.cx + PADDING * 2) as u32;
        item.itemHeight = ROW_H as u32;
        true
    })
}

pub unsafe fn draw(lparam: LPARAM) -> bool {
    let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
    STATE.with(|s| {
        let s = s.borrow();
        let Some(state) = s.as_ref() else { return false };
        let Some(text) = state.items.get(item.itemData.wrapping_sub(1)) else { return false };
        let c = state.colors;
        let dc = item.hDC;
        let background = CreateSolidBrush(colorref(c.background));
        FillRect(dc, &item.rcItem, background);
        let _ = DeleteObject(background);
        let r = item.rcItem;
        if text.is_empty() {
            let line = CreateSolidBrush(colorref(c.surface));
            let mid = (r.top + r.bottom) / 2;
            FillRect(dc, &RECT { left: r.left + 8, top: mid, right: r.right - 8, bottom: mid + 1 }, line);
            let _ = DeleteObject(line);
            return true;
        }
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        if item.itemState.0 & ODS_SELECTED.0 != 0 && !disabled {
            // Sorot membulat sedikit ke dalam, seperti baris MenuItem NotchMusic.
            let hover = CreateSolidBrush(colorref(c.surface));
            FillRect(dc, &RECT { left: r.left + 4, top: r.top + 1, right: r.right - 4, bottom: r.bottom - 1 }, hover);
            let _ = DeleteObject(hover);
        }
        let old = SelectObject(dc, state.font);
        SetBkMode(dc, TRANSPARENT);
        // Teks putih; tidak aktif = diredupkan ke arah warna latar.
        let fg = if disabled { blend(c.text, c.background, 0.55) } else { c.text };
        SetTextColor(dc, colorref(fg));
        if item.itemState.0 & ODS_CHECKED.0 != 0 {
            let mut check: Vec<u16> = "✓".encode_utf16().collect();
            let mut area = RECT { left: r.left + PADDING - 4, top: r.top, right: r.left + PADDING - 4 + CHECK_W, bottom: r.bottom };
            DrawTextW(dc, &mut check, &mut area, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        }
        let mut label = text.clone();
        let mut area = RECT { left: r.left + PADDING - 4 + CHECK_W, top: r.top, right: r.right - PADDING, bottom: r.bottom };
        DrawTextW(dc, &mut label, &mut area, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        SelectObject(dc, old);
        true
    })
}

fn blend(a: u32, b: u32, t: f32) -> u32 {
    let mix = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// Pemisah tipis berwarna permukaan tema.
pub unsafe fn separator(menu: HMENU) {
    append(menu, MF_DISABLED, 0, ""); // MF_SEPARATOR tidak digambar sendiri oleh Windows; baris mati tak bisa dipilih.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_convert_and_blend() {
        assert_eq!(colorref(0x112233).0, 0x332211);
        assert_eq!(blend(0xFFFFFF, 0x000000, 0.5), 0x808080);
    }
}
