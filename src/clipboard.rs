// Riwayat clipboard sesi: teks saja, maksimal 64 item, tidak pernah ditulis ke disk.
// Port dari ClipboardFeature.cs; isi yang ditandai pengelola sandi tidak dicatat.

use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT};

pub const MAX_ITEMS: usize = 64;
pub const MAX_TEXT_CHARS: usize = 16_384;
const MAX_TOTAL_CHARS: usize = 262_144;
const UNICODE_TEXT: u32 = 13;

pub struct Item {
    pub id: u64,
    pub text: String,
    /// Pratinjau satu baris (180 karakter, spasi/baris baru diratakan).
    pub preview: String,
    /// Jam salin, "HH:mm".
    pub time: String,
}

#[derive(Default)]
pub struct History {
    items: Vec<Item>,
    characters: usize,
    next_id: u64,
}

impl History {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Tambah teks di paling atas; duplikat pindah ke atas, bukan dobel.
    pub fn add(&mut self, text: &str, time: String) -> bool {
        let length = text.chars().count();
        if text.trim().is_empty() || length > MAX_TEXT_CHARS || text.contains('\0') {
            return false;
        }
        if let Some(i) = self.items.iter().position(|item| item.text == text) {
            let old = self.items.remove(i);
            self.characters -= old.text.chars().count();
        }
        self.next_id += 1;
        self.items.insert(0, Item { id: self.next_id, text: text.to_string(), preview: preview(text, 180), time });
        self.characters += length;
        while self.items.len() > MAX_ITEMS || self.characters > MAX_TOTAL_CHARS {
            if let Some(last) = self.items.pop() {
                self.characters -= last.text.chars().count();
            }
        }
        true
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(i) = self.items.iter().position(|item| item.id == id) else { return false };
        let old = self.items.remove(i);
        self.characters -= old.text.chars().count();
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.characters = 0;
    }

    /// Item yang memuat semua kata pencarian (tanpa peduli huruf besar/kecil), paling banyak 8 kata.
    pub fn find(&self, query: &str) -> Vec<&Item> {
        let query: String = query.chars().take(256).collect::<String>().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().take(8).collect();
        self.items.iter().filter(|item| {
            let text = item.text.to_lowercase();
            words.iter().all(|w| text.contains(w))
        }).collect()
    }
}

fn preview(text: &str, maximum: usize) -> String {
    let mut out: String = text.chars().take(maximum).map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
    if text.chars().count() > maximum {
        out.push('…');
    }
    out
}

pub enum Read {
    /// Clipboard sedang dipakai aplikasi lain: coba lagi sebentar.
    Busy,
    Skip,
    TooLarge,
    Text(String),
}

struct Formats {
    own: u32,
    excluded: u32,
    history: u32,
    cloud: u32,
}

fn formats() -> &'static Formats {
    static FORMATS: OnceLock<Formats> = OnceLock::new();
    FORMATS.get_or_init(|| unsafe {
        Formats {
            own: RegisterClipboardFormatW(w!("NotchRust.SessionClipboard.v1")),
            excluded: RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            history: RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
            cloud: RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
        }
    })
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn read(owner: HWND) -> Read {
    let f = formats();
    unsafe {
        if OpenClipboard(owner).is_err() {
            return Read::Busy;
        }
        let result = (|| {
            let available = |format: u32| IsClipboardFormatAvailable(format).is_ok();
            // Salinan dari notch sendiri, pengelola sandi, atau yang melarang riwayat: lewati.
            if available(f.own) || available(f.excluded) || (available(f.history) && zero_dword(f.history)) || !available(UNICODE_TEXT) {
                return Read::Skip;
            }
            let Ok(handle) = GetClipboardData(UNICODE_TEXT) else { return Read::Skip };
            let global = HGLOBAL(handle.0);
            let count = (GlobalSize(global) / 2).min(MAX_TEXT_CHARS + 1);
            if count == 0 {
                return Read::Skip;
            }
            let pointer = GlobalLock(global) as *const u16;
            if pointer.is_null() {
                return Read::Skip;
            }
            let units = std::slice::from_raw_parts(pointer, count);
            let length = units.iter().position(|&u| u == 0).unwrap_or(count);
            let result = if length > MAX_TEXT_CHARS || length == count {
                Read::TooLarge
            } else {
                Read::Text(String::from_utf16_lossy(&units[..length]))
            };
            let _ = GlobalUnlock(global);
            result
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Teks clipboard untuk ditempel ke kotak teks (Ctrl+V), tanpa saringan riwayat.
pub fn paste(owner: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(owner).ok()?;
        let text = (|| {
            let handle = GetClipboardData(UNICODE_TEXT).ok()?;
            let global = HGLOBAL(handle.0);
            let count = (GlobalSize(global) / 2).min(MAX_TEXT_CHARS);
            let pointer = GlobalLock(global) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let units = std::slice::from_raw_parts(pointer, count);
            let length = units.iter().position(|&u| u == 0).unwrap_or(count);
            let text = String::from_utf16_lossy(&units[..length]);
            let _ = GlobalUnlock(global);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

unsafe fn zero_dword(format: u32) -> bool {
    let Ok(handle) = GetClipboardData(format) else { return true };
    let global = HGLOBAL(handle.0);
    if GlobalSize(global) < 4 {
        return true;
    }
    let pointer = GlobalLock(global) as *const u32;
    if pointer.is_null() {
        return true;
    }
    let value = pointer.read_unaligned();
    let _ = GlobalUnlock(global);
    value == 0
}

/// Salin kembali ke clipboard. Ditandai milik notch, dan tidak masuk riwayat/cloud clipboard Windows.
pub fn write(owner: HWND, text: &str) -> bool {
    let f = formats();
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if units.len() > MAX_TEXT_CHARS * 2 {
        return false;
    }
    unsafe {
        let allocate = |bytes: usize| GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes).ok();
        let (Some(data), Some(marker), Some(history), Some(cloud)) =
            (allocate(units.len() * 2), allocate(4), allocate(4), allocate(4))
        else {
            return false;
        };
        let mut owned = vec![data, marker, history, cloud];
        let fill = |global: HGLOBAL, bytes: &[u8]| {
            let pointer = GlobalLock(global) as *mut u8;
            if !pointer.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
                let _ = GlobalUnlock(global);
            }
        };
        fill(data, std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2));
        fill(marker, &1u32.to_le_bytes());
        let mut ok = false;
        if OpenClipboard(owner).is_ok() {
            if EmptyClipboard().is_ok() && SetClipboardData(UNICODE_TEXT, HANDLE(data.0)).is_ok() {
                ok = true;
                owned.retain(|g| g.0 != data.0); // Windows mengambil alih memori yang berhasil dipasang.
                for (format, global) in [(f.own, marker), (f.history, history), (f.cloud, cloud)] {
                    if SetClipboardData(format, HANDLE(global.0)).is_ok() {
                        owned.retain(|g| g.0 != global.0);
                    }
                }
            }
            let _ = CloseClipboard();
        }
        for global in owned {
            let _ = GlobalFree(global);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_and_respects_limits() {
        let mut history = History::default();
        assert!(!history.add("   ", String::new()));
        assert!(!history.add(&"x".repeat(MAX_TEXT_CHARS + 1), String::new()));
        for i in 0..MAX_ITEMS + 5 {
            history.add(&format!("teks {i}"), String::new());
        }
        history.add("teks 10", String::new()); // Duplikat pindah ke paling atas, tidak dobel.
        assert_eq!(history.items().len(), MAX_ITEMS);
        assert_eq!(history.items()[0].text, "teks 10");
        assert_eq!(history.items().iter().filter(|x| x.text == "teks 10").count(), 1);
    }

    #[test]
    fn find_matches_all_words_case_insensitively() {
        let mut history = History::default();
        history.add("Rapat Senin jam 9", String::new());
        history.add("Belanja sayur", String::new());
        assert_eq!(history.find("senin RAPAT").len(), 1);
        assert_eq!(history.find("").len(), 2);
        assert_eq!(history.find("kamis").len(), 0);
    }
}
