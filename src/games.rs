// Peluncur game (port GameFeature.cs NotchMusic): daftar pilihan sendiri (appid Steam atau path shortcut),
// tombol Scan untuk game Steam yang terakhir dimainkan, ikon dari shortcut, dan membuka game.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::{
    SHGetKnownFolderPath, ShellExecuteW, FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs,
    FOLDERID_PublicDesktop, KNOWN_FOLDER_FLAG,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Jumlah game maksimal (kotak ke-8 adalah "+").
pub const SLOTS: usize = 7;
/// Aplikasi Steam yang bukan game: Wallpaper Engine, Steamworks Redistributables, SteamVR.
/// shortcut: daftar tetap, tambah appid di sini bila ada aplikasi non-game lain yang ikut ter-scan.
const NOT_GAMES: [&str; 3] = ["431960", "228980", "250820"];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Steam,
    Custom,
}

#[derive(Clone, PartialEq, Debug)]
pub enum IconSource {
    /// File .ico (shortcut Steam di Desktop/Start).
    Ico(PathBuf),
    /// Gambar kecil di cache Steam.
    Image(PathBuf),
    /// Ikon seperti di Explorer untuk shortcut/aplikasi.
    Shell(String),
    None,
}

#[derive(Clone, Debug)]
pub struct Game {
    /// appid Steam atau path shortcut.
    pub key: String,
    pub name: String,
    pub kind: Kind,
    pub icon: IconSource,
}

pub fn is_app_id(key: &str) -> bool {
    !key.is_empty() && key.len() <= 10 && key.bytes().all(|b| b.is_ascii_digit())
}

/// Kunci tersimpan yang masih valid: appid atau path absolut, tanpa duplikat, maksimal SLOTS.
pub fn clean_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = Vec::<String>::new();
    for key in keys.into_iter().map(str::trim) {
        let valid = is_app_id(key) || (key.len() <= 1024 && Path::new(key).is_absolute());
        if valid && !seen.iter().any(|k| k.eq_ignore_ascii_case(key)) {
            seen.push(key.to_string());
        }
    }
    seen.truncate(SLOTS);
    seen
}

const SETTINGS_KEY: PCWSTR = w!("Software\\NotchRust");

pub fn load_keys() -> Vec<String> {
    let mut buffer = vec![0u16; 16 * 1024];
    let mut size = (buffer.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, w!("Games"), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr() as _), Some(&mut size))
    }
    .is_ok();
    if !ok {
        return notchmusic_keys();
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(0);
    clean_keys(String::from_utf16_lossy(&buffer[..length]).split('\n'))
}

/// Pertama kali jalan: pakai daftar game NotchMusic (GameOrder di ui-settings.json) bila ada.
fn notchmusic_keys() -> Vec<String> {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(Path::new(&base).join("NotchMusic").join("ui-settings.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let keys: Vec<&str> = json["GameOrder"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    clean_keys(keys)
}

pub fn save_keys(keys: &[String]) {
    let text: Vec<u16> = keys.join("\n").encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, w!("Games"), REG_SZ.0, Some(text.as_ptr() as _), (text.len() * 2) as u32);
    }
}

fn steam_path() -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath"), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr() as _), Some(&mut size))
    }
    .ok()
    .ok()?;
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(0);
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length]).replace('/', "\\"));
    path.is_dir().then_some(path)
}

pub struct SteamGame {
    pub id: String,
    pub name: String,
    pub last_played: i64,
}

fn value(text: &str, key: &str) -> Option<String> {
    static PAIR: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pair = PAIR.get_or_init(|| Regex::new(r#""([^"]+)"\s*"([^"]*)""#).expect("regex"));
    let found = pair.captures_iter(text).find(|c| c[1].eq_ignore_ascii_case(key))?;
    let value = found[2].trim();
    (!value.is_empty()).then(|| value.chars().take(128).collect())
}

/// Folder library tambahan (mis. D:\Games\Steam) tercatat di libraryfolders.vdf.
fn library_paths(vdf: &str) -> Vec<String> {
    let pattern = Regex::new(r#""path"\s*"([^"]+)""#).expect("regex");
    pattern.captures_iter(vdf).map(|c| c[1].replace("\\\\", "\\")).collect()
}

/// Game Steam terpasang yang pernah dimainkan, terbaru dulu.
pub fn steam_games() -> Vec<SteamGame> {
    let Some(steam) = steam_path() else { return Vec::new() };
    let mut libraries = vec![steam.to_string_lossy().to_string()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        libraries.extend(library_paths(&vdf));
    }
    libraries.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let mut found: Vec<SteamGame> = Vec::new();
    for library in libraries {
        let Ok(entries) = std::fs::read_dir(Path::new(&library).join("steamapps")) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) || entry.metadata().map_or(true, |m| m.len() > 256 * 1024) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let (Some(id), Some(title)) = (value(&text, "appid"), value(&text, "name")) else { continue };
            let played = value(&text, "LastPlayed").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            // Belum pernah dimainkan (mis. Steamworks Redistributables) tidak ditampilkan.
            if is_app_id(&id) && played > 0 && !NOT_GAMES.contains(&id.as_str()) && !found.iter().any(|g| g.id == id) {
                found.push(SteamGame { id, name: title, last_played: played });
            }
        }
    }
    found.sort_by(|a, b| b.last_played.cmp(&a.last_played));
    found
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let path: PWSTR = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as _));
        text.map(PathBuf::from)
    }
}

/// Ikon shortcut Steam (sama dengan ikon di Desktop): .url berisi steam://rungameid/<id> dan IconFile=<.ico>.
fn shortcut_icons() -> HashMap<String, PathBuf> {
    let id_pattern = Regex::new(r"(?i)steam://rungameid/([0-9]{1,10})").expect("regex");
    let icon_pattern = Regex::new(r"(?im)^IconFile=(.+\.ico)\s*$").expect("regex");
    let mut icons = HashMap::new();
    for folder in [&FOLDERID_Desktop, &FOLDERID_PublicDesktop, &FOLDERID_Programs, &FOLDERID_CommonPrograms].iter().filter_map(|id| known_folder(id)) {
        let mut stack = vec![(folder, 0)];
        let mut seen = 0;
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if depth < 3 {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("url")) || entry.metadata().map_or(true, |m| m.len() > 4096) {
                    continue;
                }
                seen += 1;
                if seen > 500 {
                    break;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if let (Some(id), Some(icon)) = (id_pattern.captures(&text), icon_pattern.captures(&text)) {
                    let icon = PathBuf::from(icon[1].trim());
                    if icon.is_file() {
                        icons.entry(id[1].to_string()).or_insert(icon);
                    }
                }
            }
        }
    }
    icons
}

/// Ikon 32x32 di cache Steam bila belum ada shortcut.
fn cache_icon(id: &str) -> Option<PathBuf> {
    let folder = steam_path()?.join("appcache").join("librarycache").join(id);
    let hash = Regex::new(r"^[0-9a-f]{40}\.jpg$").expect("regex");
    std::fs::read_dir(folder).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| hash.is_match(&n.to_string_lossy())))
}

/// Kunci tersimpan -> game lengkap (nama + sumber ikon). Game Steam yang sudah di-uninstall dibuang,
/// kecuali Steam sama sekali tidak terbaca (agar daftar tidak terhapus).
pub fn resolve(keys: &[String]) -> Vec<Game> {
    let steam = if keys.iter().any(|k| is_app_id(k)) { steam_games() } else { Vec::new() };
    let icons = if keys.iter().any(|k| is_app_id(k)) { shortcut_icons() } else { HashMap::new() };
    keys.iter()
        .filter_map(|key| {
            if is_app_id(key) {
                let name = steam.iter().find(|g| &g.id == key).map(|g| g.name.clone());
                if name.is_none() && !steam.is_empty() {
                    return None;
                }
                let icon = match icons.get(key) {
                    Some(ico) => IconSource::Ico(ico.clone()),
                    None => cache_icon(key).map_or(IconSource::None, IconSource::Image),
                };
                Some(Game { key: key.clone(), name: name.unwrap_or_else(|| format!("Steam {key}")), kind: Kind::Steam, icon })
            } else {
                let path = Path::new(key);
                let name = path.file_stem().map_or_else(|| key.clone(), |s| s.to_string_lossy().to_string());
                let icon = if path.exists() { IconSource::Shell(key.clone()) } else { IconSource::None };
                Some(Game { key: key.clone(), name, kind: Kind::Custom, icon })
            }
        })
        .collect()
}

pub fn launch(game: &Game) -> bool {
    let target = match game.kind {
        Kind::Steam if is_app_id(&game.key) => format!("steam://rungameid/{}", game.key),
        Kind::Custom if Path::new(&game.key).exists() => game.key.clone(),
        _ => return false,
    };
    let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let result = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(target.as_ptr()), None, None, SW_SHOWNORMAL) };
    result.0 as isize > 32
}

/// Posisi baru setelah ikon diseret ke kotak `to` (indeks kotak "+" = jumlah game = paling belakang).
pub fn reorder(keys: &mut Vec<String>, from: usize, to: usize) -> bool {
    let to = to.min(keys.len() - 1);
    if from >= keys.len() || from == to {
        return false;
    }
    let key = keys.remove(from);
    keys.insert(to, key);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated_and_capped() {
        let keys = clean_keys(["570", "570", "bukan path", "12345678901", r"C:\Games\A.lnk", r"c:\games\a.lnk", "1", "2", "3", "4", "5", "6"]);
        assert_eq!(keys, vec!["570", r"C:\Games\A.lnk", "1", "2", "3", "4", "5"]);
    }

    #[test]
    fn steam_manifest_values_and_libraries() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"LastPlayed\"\t\t\"1761800000\"\n}";
        assert_eq!(value(acf, "appid").as_deref(), Some("570"));
        assert_eq!(value(acf, "name").as_deref(), Some("Dota 2"));
        assert_eq!(value(acf, "lastplayed").as_deref(), Some("1761800000"));
        let vdf = "\"0\" { \"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\" } \"1\" { \"path\" \"D:\\\\SteamLibrary\" }";
        assert_eq!(library_paths(vdf), vec![r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
    }

    #[test]
    fn reorder_moves_and_add_tile_means_last() {
        let mut keys: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert!(reorder(&mut keys, 0, 2));
        assert_eq!(keys, ["b", "c", "a"]);
        assert!(reorder(&mut keys, 2, 0));
        assert_eq!(keys, ["a", "b", "c"]);
        assert!(reorder(&mut keys, 0, 3)); // Dilepas di kotak "+" = paling belakang.
        assert_eq!(keys, ["b", "c", "a"]);
        assert!(!reorder(&mut keys, 1, 1));
    }
}
