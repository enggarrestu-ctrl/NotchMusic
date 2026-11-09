// NotchRust: widget notch di atas layar Windows, port Rust dari NotchMusic (WPF).
// Saat diam: logo, jam, kecepatan internet (atau baris lirik saat lagu diputar).
// Saat di-hover: panel musik + lirik, dan halaman riwayat clipboard.
#![windows_subsystem = "windows"]

mod clipboard;
mod draw;
mod http;
mod lyrics;
mod lyrics_window;
mod media;
mod net;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{
    BOOL, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use draw::{ClipHit, ClipRow, ClipView, Gfx, Hit, LyricView, View, COMPACT_H, COMPACT_W, EXPANDED_H, EXPANDED_W, THEMES};
use media::{Command, MediaState};
use net::Speed;

const WM_MEDIA: u32 = WM_APP + 1;
const WM_NET: u32 = WM_APP + 2;
const WM_TRAY: u32 = WM_APP + 3;
const WM_LYRICS: u32 = WM_APP + 4;
const WM_LYRIC_CHOICES: u32 = WM_APP + 5;
// Tidak tersedia di modul WindowsAndMessaging crate windows.
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;

const TIMER_ANIMATION: usize = 1;
const TIMER_TICK: usize = 2;
const TIMER_COLLAPSE: usize = 3;
const TIMER_EQUALIZER: usize = 4;
const TIMER_CLIPBOARD_RETRY: usize = 5;

const ANIMATION_SECONDS: f32 = 0.24;
const PAGE_SECONDS: f32 = 0.24;
const COLLAPSE_DELAY_MS: u32 = 350;

const MENU_AUTOSTART: usize = 1;
const MENU_EXIT: usize = 2;
const MENU_SHOW: usize = 3;
const MENU_OFFLINE: usize = 4;
const MENU_REFRESH_LYRICS: usize = 5;
const MENU_LYRICS_WINDOW: usize = 6;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("NotchRust");
const SETTINGS_KEY: PCWSTR = w!("Software\\NotchRust");
/// Tema bawaan bila belum ada pilihan: Hitam Legam.
const DEFAULT_THEME: usize = 5;

const DAYS: [&str; 7] = ["Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu"];
const MONTHS: [&str; 12] = [
    "Januari", "Februari", "Maret", "April", "Mei", "Juni",
    "Juli", "Agustus", "September", "Oktober", "November", "Desember",
];

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct App {
    hwnd: HWND,
    gfx: Gfx,
    dpi: f32,
    origin: POINT,
    /// Posisi animasi saat ini (0 = kecil, 1 = terbuka), sudah di-ease.
    t: f32,
    from: f32,
    target: f32,
    started: Instant,
    tracking: bool,
    hover: Hit,
    media: Arc<Mutex<MediaState>>,
    speed: Arc<Mutex<Speed>>,
    commands: Sender<Command>,
    tray_icon: HICON,
    theme: usize,
    pinned: bool,
    /// Diminimalkan ke tray: jendela disembunyikan, ikon tray tetap ada.
    hidden: bool,
    /// Ukuran notch kecil saat ini dan tujuannya (melebar saat menampilkan lirik).
    compact: (f32, f32),
    compact_target: (f32, f32),
    lyrics: Arc<Mutex<lyrics::Shared>>,
    lyric_requests: Sender<lyrics::Request>,
    /// Lagu yang liriknya terakhir diminta.
    lyric_key: String,
    offline: bool,
    page: usize,
    /// Halaman asal dan arah selama animasi geser halaman.
    page_transition: Option<(usize, f32, Instant)>,
    history: clipboard::History,
    recording: bool,
    clipboard_listening: bool,
    clipboard_sequence: u32,
    clipboard_retries: u32,
    clipboard_status: String,
    search: String,
    search_focused: bool,
    scroll: f32,
    started_at: Instant,
    instance: HINSTANCE,
    lyrics_window: Option<lyrics_window::LyricsWindow>,
    lyrics_mode: lyrics::Mode,
    /// Hasil pencarian manual terakhir untuk daftar "Lirik yang dipakai".
    choices: Vec<lyrics::Choice>,
    chosen: Option<usize>,
    choices_result: Arc<Mutex<Option<std::result::Result<Vec<lyrics::Choice>, String>>>>,
    /// Lagu yang ditampilkan jendela lirik (untuk mengisi ulang judul/artis saat lagu berganti).
    window_key: String,
    window_note: String,
    searching: bool,
}

#[derive(Default)]
struct LyricLines {
    previous: String,
    current: String,
    next: String,
    compact: Option<String>,
}

fn track_key(media: &MediaState) -> String {
    format!("{}\u{1}{}\u{1}{}", media.title, media.artist, media.album)
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Akses state UI. Bila sedang dipakai (mis. pesan masuk saat menu modal), lewati saja.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _instance = CreateMutexW(None, true, w!("Local\\NotchRust.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        let module = GetModuleHandleW(None)?;
        let instance: HINSTANCE = module.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("NotchRustWindow"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        // Selalu di atas, tidak muncul di Alt+Tab/taskbar, dan tidak mengambil fokus saat diklik.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("NotchRustWindow"),
            w!("NotchRust"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            instance,
            None,
        )?;

        let media = Arc::new(Mutex::new(MediaState::default()));
        let speed = Arc::new(Mutex::new(Speed::default()));
        let commands = media::spawn(hwnd.0 as isize, WM_MEDIA, media.clone());
        net::spawn(hwnd.0 as isize, WM_NET, speed.clone());
        let lyrics_shared = Arc::new(Mutex::new(lyrics::Shared::default()));
        let lyric_requests = lyrics::spawn(hwnd.0 as isize, WM_LYRICS, lyrics_shared.clone());

        let app = App {
            hwnd,
            gfx: Gfx::new()?,
            dpi: 96.0,
            origin: POINT::default(),
            t: 0.0,
            from: 0.0,
            target: 0.0,
            started: Instant::now(),
            tracking: false,
            hover: Hit::None,
            media,
            speed,
            commands,
            tray_icon: load_tray_icon(),
            theme: read_setting("Theme").map(|v| v as usize).or_else(notchmusic_theme).unwrap_or(DEFAULT_THEME) % THEMES.len(),
            pinned: read_setting("Pinned").map(|v| v != 0).unwrap_or(true),
            hidden: false,
            compact: (COMPACT_W, COMPACT_H),
            compact_target: (COMPACT_W, COMPACT_H),
            lyrics: lyrics_shared,
            lyric_requests,
            lyric_key: String::new(),
            offline: read_setting("LyricsOffline").is_some_and(|v| v != 0),
            page: read_setting("Page").map(|v| v as usize % draw::PAGE_COUNT).unwrap_or(draw::PAGE_MUSIC),
            page_transition: None,
            history: clipboard::History::default(),
            recording: read_setting("ClipboardRecording").map(|v| v != 0).unwrap_or(true),
            clipboard_listening: false,
            // Jangan mengimpor isi clipboard lama saat mulai: sesi baru selalu kosong.
            clipboard_sequence: clipboard::sequence(),
            clipboard_retries: 0,
            clipboard_status: "Riwayat sesi · teks saja".into(),
            search: String::new(),
            search_focused: false,
            scroll: 0.0,
            started_at: Instant::now(),
            instance,
            lyrics_window: None,
            lyrics_mode: lyrics::Mode::from_index(read_setting("LyricsMode").unwrap_or(0)),
            choices: Vec::new(),
            chosen: None,
            choices_result: Arc::new(Mutex::new(None)),
            window_key: String::new(),
            window_note: String::new(),
            searching: false,
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        with_app(|app| {
            app.layout();
            app.render();
            app.add_tray();
            app.apply_pin();
            app.clipboard_listening = AddClipboardFormatListener(app.hwnd).is_ok();
            if !app.clipboard_listening {
                app.clipboard_status = "Pencatatan belum aktif. Tutup dan buka aplikasi lagi.".into();
            }
        });
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_TICK, 1000, None);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        APP.with(|cell| cell.borrow_mut().take());
        Ok(())
    }
}

impl App {
    /// Letakkan notch di tengah atas monitor utama, sesuai DPI monitor tersebut.
    fn layout(&mut self) {
        unsafe {
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.dpi = dpi_x as f32;
            let scale = self.dpi / 96.0;
            let width = (EXPANDED_W * scale).ceil() as i32;
            let height = (EXPANDED_H * scale).ceil() as i32;
            let screen = info.rcMonitor;
            self.origin = POINT { x: screen.left + (screen.right - screen.left - width) / 2, y: screen.top };
            let _ = self.gfx.resize(width, height, self.dpi);
        }
    }

    fn render(&mut self) {
        let media = self.media.lock().unwrap().clone();
        let speed = *self.speed.lock().unwrap();
        let time = unsafe { GetLocalTime() };
        let clock = format!("{:02}:{:02}", time.wHour, time.wMinute);
        let date = format!(
            "{}, {:02} {} {}",
            DAYS[time.wDayOfWeek as usize % 7],
            time.wDay,
            MONTHS[(time.wMonth as usize).clamp(1, 12) - 1],
            time.wYear
        );
        let down = net::format_speed(speed.down);
        let up = net::format_speed(speed.up);
        let now = now_ticks();
        let lines = self.lyric_lines(&media, now);

        // Notch kecil melebar mengikuti panjang baris lirik, seperti MusicMinWidth + CompactChrome.
        let compact_line = lines.compact.as_deref();
        self.compact_target = match compact_line {
            Some(line) => (
                (self.gfx.compact_lyric_width(line) + draw::COMPACT_CHROME).clamp(COMPACT_W, EXPANDED_W),
                draw::MUSIC_H,
            ),
            None => (COMPACT_W, COMPACT_H),
        };
        if self.compact != self.compact_target {
            self.start_animation_timer();
        }
        self.update_equalizer(compact_line.is_some());

        let rows: Vec<ClipRow> = self
            .history
            .find(&self.search)
            .into_iter()
            .map(|item| ClipRow { id: item.id, preview: &item.preview, time: &item.time })
            .collect();
        let caret = (self.started_at.elapsed().as_millis() / 530) % 2 == 0;
        let transition = self.page_transition.map(|(from, direction, started)| {
            let p = (started.elapsed().as_secs_f32() / PAGE_SECONDS).min(1.0);
            (from, direction, 1.0 - (1.0 - p).powi(3))
        });
        let elapsed_secs = self.started_at.elapsed().as_secs_f32();
        let eq = std::array::from_fn(|i| {
            let speed = [5.1, 6.7, 4.3, 7.9, 5.9][i];
            let phase = [0.0, 1.3, 2.1, 0.7, 2.9][i];
            0.35 + 0.65 * (elapsed_secs * speed + phase).sin().abs()
        });
        let view = View {
            t: self.t,
            compact: self.compact,
            clock: &clock,
            date: &date,
            down: &down,
            up: &up,
            media: &media,
            now,
            hover: self.hover,
            theme: &THEMES[self.theme],
            pinned: self.pinned,
            lyric: LyricView {
                previous: &lines.previous,
                current: &lines.current,
                next: &lines.next,
                compact: compact_line,
                eq,
            },
            clip: ClipView {
                rows,
                total: self.history.items().len(),
                recording: self.recording,
                search: &self.search,
                focused: self.search_focused,
                caret,
                status: &self.clipboard_status,
                scroll: self.scroll,
            },
            page: self.page,
            transition,
        };
        let _ = self.gfx.present(self.hwnd, self.origin, &view, self.dpi);
    }

    /// Baris lirik untuk lagu saat ini: sebelumnya, sekarang, berikutnya, dan baris notch kecil.
    fn lyric_lines(&mut self, media: &MediaState, now: i64) -> LyricLines {
        let mut lines = LyricLines::default();
        if !media.active {
            lines.current = "Belum ada lagu".into();
            return lines;
        }
        let key = track_key(media);
        if key != self.lyric_key {
            self.request_lyrics(media, false);
        }
        let shared = self.lyrics.lock().unwrap().clone();
        let state = if shared.key == key { shared.state } else { lyrics::State::Searching };
        match state {
            lyrics::State::Idle | lyrics::State::Searching => lines.current = "Mencari lirik…".into(),
            lyrics::State::Missing(message) => lines.current = message,
            lyrics::State::Found(found) if found.is_synced() => {
                // Sedikit lebih awal agar baris tidak terasa terlambat dari suara; offset dari jendela lirik.
                let index = found.index_at(media.elapsed_secs(now) + 0.15 - found.offset);
                let text = |i: Option<usize>| i.and_then(|i| found.lines.get(i)).map(|l| l.text.clone()).unwrap_or_default();
                lines.previous = text(index.and_then(|i| i.checked_sub(1)));
                lines.current = text(index);
                lines.next = text(Some(index.map_or(0, |i| i + 1)));
                if lines.current.trim().is_empty() {
                    lines.current = "♪".into();
                }
                if media.playing {
                    lines.compact = Some(lines.current.clone());
                }
            }
            lyrics::State::Found(found) if found.instrumental => lines.current = "Instrumental — tidak ada lirik.".into(),
            lyrics::State::Found(found) => {
                lines.previous = "Lirik tanpa waktu".into();
                lines.current = found.plain.first().cloned().unwrap_or_default();
                lines.next = found.plain.get(1).cloned().unwrap_or_default();
            }
        }
        lines
    }

    fn request_lyrics(&mut self, media: &MediaState, refresh: bool) {
        self.lyric_key = track_key(media);
        let _ = self.lyric_requests.send(lyrics::Request {
            key: self.lyric_key.clone(),
            title: media.title.clone(),
            artist: media.artist.clone(),
            album: media.album.clone(),
            duration: media.duration_secs(),
            offline: self.offline,
            refresh,
            mode: self.lyrics_mode,
        });
    }

    // ---- Jendela lirik (Aa) ------------------------------------------------------------------

    fn open_lyrics_window(&mut self) {
        if let Some(window) = &self.lyrics_window {
            unsafe {
                let _ = ShowWindow(window.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(window.hwnd);
            }
            return;
        }
        let theme = &THEMES[self.theme];
        let opened = lyrics_window::LyricsWindow::open(self.instance, theme, self.pinned, self.dpi, on_lyrics_action);
        if let Ok(window) = opened {
            window.set_mode(self.lyrics_mode.index());
            self.lyrics_window = Some(window);
            self.window_key.clear();
            self.refresh_lyrics_window();
            // Belum ada daftar hasil untuk lagu ini: langsung cari agar bisa dipilih.
            if self.choices.is_empty() && !self.searching {
                self.search_choices();
            }
        }
    }

    /// Lirik yang sedang dipakai untuk lagu saat ini (bila ada).
    fn current_lyrics(&self, media: &MediaState) -> Option<Arc<lyrics::Lyrics>> {
        let shared = self.lyrics.lock().unwrap();
        match &shared.state {
            lyrics::State::Found(found) if shared.key == track_key(media) => Some(found.clone()),
            _ => None,
        }
    }

    fn refresh_lyrics_window(&mut self) {
        let Some(window) = &self.lyrics_window else { return };
        let media = self.media.lock().unwrap().clone();
        let key = track_key(&media);
        if key != self.window_key {
            // Lagu berganti: isi ulang judul/artis pencarian dan kosongkan daftar hasil lama.
            self.window_key = key.clone();
            let (title, artist) = lyrics::clean_query(&media.title, &media.artist);
            window.set_query(&title, &artist);
            if !self.searching {
                self.choices.clear();
                self.chosen = None;
                self.window_note.clear();
            }
            let labels: Vec<String> = self.choices.iter().map(|c| c.label.clone()).collect();
            window.set_choices(&labels, self.chosen);
        }
        window.set_heading(&if media.active { format!("{} — {}", media.title, media.artist) } else { "Belum ada lagu".into() });
        let current = self.current_lyrics(&media);
        let state_text = {
            let shared = self.lyrics.lock().unwrap();
            match &shared.state {
                _ if shared.key != key => "Mencari lirik…".to_string(),
                lyrics::State::Idle | lyrics::State::Searching => "Mencari lirik…".into(),
                lyrics::State::Missing(message) => message.clone(),
                lyrics::State::Found(found) => format!("Dipakai: {}", found.source),
            }
        };
        let status = if self.window_note.is_empty() { state_text } else { format!("{} {}", self.window_note, state_text) };
        window.set_status(&status);
        window.set_offset(current.as_ref().map_or(0.0, |l| l.offset));
        window.set_can_save(current.is_some(), current.as_ref().is_some_and(|l| l.is_synced()));
        window.set_can_load(media.active);
        window.set_body(&current.map_or("Pilih hasil yang cocok untuk menampilkan teks di notch.".to_string(), |l| l.text()));
    }

    /// Cari kandidat dengan judul/artis dari jendela lirik, di thread terpisah.
    fn search_choices(&mut self) {
        let Some(window) = &self.lyrics_window else { return };
        let (title, artist) = window.query();
        if title.is_empty() {
            self.window_note = "Isi judul lagu dulu.".into();
            self.refresh_lyrics_window();
            return;
        }
        if self.offline {
            self.window_note = "Mode lirik offline aktif; matikan di menu klik kanan untuk mencari.".into();
            self.refresh_lyrics_window();
            return;
        }
        self.searching = true;
        self.window_note = "Mencari di LRCLIB…".into();
        let media = self.media.lock().unwrap().clone();
        let (duration, result, hwnd) = (media.duration_secs(), self.choices_result.clone(), self.hwnd.0 as isize);
        // Judul/artis bawaan (belum diubah): kenali lagu asli dari judul video dulu, seperti pencarian otomatis.
        let identify = (title.clone(), artist.clone()) == lyrics::clean_query(&media.title, &media.artist);
        let (raw_title, raw_artist) = if identify { (media.title.clone(), media.artist.clone()) } else { (title, artist) };
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
            }
            let found = lyrics::search_choices(&raw_title, &raw_artist, &media.album, duration, identify);
            *result.lock().unwrap() = Some(found);
            unsafe {
                let _ = PostMessageW(HWND(hwnd as _), WM_LYRIC_CHOICES, WPARAM(0), LPARAM(0));
            }
        });
        self.refresh_lyrics_window();
    }

    fn choices_ready(&mut self) {
        let Some(result) = self.choices_result.lock().unwrap().take() else { return };
        self.searching = false;
        match result {
            Ok(choices) if choices.is_empty() => {
                self.choices.clear();
                self.chosen = None;
                self.window_note = "Tidak ada hasil. Ubah judul/artis di Lainnya lalu Cari ulang, atau Muat file .lrc.".into();
            }
            Ok(choices) => {
                let automatic = choices.iter().position(|c| c.automatic);
                self.choices = choices;
                self.chosen = None;
                self.window_note = format!("{} hasil.", self.choices.len());
                match automatic {
                    Some(index) => self.choose(index),
                    None => self.window_note.push_str(" Belum ada yang pasti cocok — pilih dari daftar."),
                }
            }
            Err(message) => self.window_note = message,
        }
        if let Some(window) = &self.lyrics_window {
            let labels: Vec<String> = self.choices.iter().map(|c| c.label.clone()).collect();
            window.set_choices(&labels, self.chosen);
        }
        self.refresh_lyrics_window();
    }

    /// Pakai lirik untuk lagu saat ini: tampil di notch dan disimpan untuk diputar berikutnya.
    fn use_lyrics(&mut self, lyrics: lyrics::Lyrics) {
        let media = self.media.lock().unwrap().clone();
        if !media.active {
            return;
        }
        let key = track_key(&media);
        lyrics::save(&lyrics::identity_of(&media.title, &media.artist, &media.album), &lyrics);
        {
            let mut shared = self.lyrics.lock().unwrap();
            shared.key = key.clone();
            shared.state = lyrics::State::Found(Arc::new(lyrics));
        }
        self.lyric_key = key;
        self.refresh_lyrics_window();
        self.render();
    }

    fn choose(&mut self, index: usize) {
        let Some(choice) = self.choices.get(index) else { return };
        let mut chosen = choice.lyrics.clone();
        lyrics::apply_mode(&mut chosen, self.lyrics_mode);
        self.chosen = Some(index);
        self.use_lyrics(chosen);
    }

    fn shift_offset(&mut self, delta: Option<f64>) {
        let media = self.media.lock().unwrap().clone();
        let Some(current) = self.current_lyrics(&media) else { return };
        let mut updated = (*current).clone();
        updated.offset = delta.map_or(0.0, |d| (updated.offset + d).clamp(-600.0, 600.0));
        self.use_lyrics(updated);
    }

    fn save_lyrics_file(&mut self) {
        let media = self.media.lock().unwrap().clone();
        let Some(current) = self.current_lyrics(&media) else { return };
        let (extension, filter) = if current.is_synced() {
            ("lrc", "Lirik berwaktu (*.lrc)\0*.lrc\0")
        } else {
            ("txt", "Teks (*.txt)\0*.txt\0")
        };
        let name = format!("{} - {}.{extension}", media.artist, media.title)
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect::<String>();
        if let Some(path) = file_dialog(self.lyrics_window.as_ref().map(|w| w.hwnd), filter, extension, &name, true) {
            self.window_note = match std::fs::write(&path, current.to_file()) {
                Ok(()) => "Lirik disimpan.".into(),
                Err(_) => "File belum bisa disimpan.".into(),
            };
            self.refresh_lyrics_window();
        }
    }

    fn load_lyrics_file(&mut self) {
        let owner = self.lyrics_window.as_ref().map(|w| w.hwnd);
        let Some(path) = file_dialog(owner, "Lirik (*.lrc;*.txt)\0*.lrc;*.txt\0Semua file\0*.*\0", "lrc", "", false) else { return };
        let loaded = std::fs::read(&path).ok().and_then(|bytes| {
            // UTF-16 (BOM FF FE) dari Notepad lama, selain itu UTF-8.
            let text = if bytes.starts_with(&[0xFF, 0xFE]) {
                let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            lyrics::Lyrics::from_file(&text)
        });
        match loaded {
            Some(mut lyrics) => {
                lyrics::apply_mode(&mut lyrics, self.lyrics_mode);
                self.chosen = None;
                if let Some(window) = &self.lyrics_window {
                    window.set_choices(&self.choices.iter().map(|c| c.label.clone()).collect::<Vec<_>>(), None);
                }
                self.window_note = "File lirik dipakai.".into();
                self.use_lyrics(lyrics);
            }
            None => {
                self.window_note = "File tidak berisi lirik yang bisa dibaca.".into();
                self.refresh_lyrics_window();
            }
        }
    }

    /// "Lirik tersimpan…": pilih dari lirik yang pernah disimpan, paling mirip judul/artis lebih dulu.
    fn show_saved_lyrics(&mut self) {
        let Some(window) = &self.lyrics_window else { return };
        if !self.media.lock().unwrap().active {
            return;
        }
        let (title, artist) = window.query();
        self.choices = lyrics::saved_choices(&title, &artist);
        self.chosen = None;
        self.window_note = if self.choices.is_empty() {
            "Belum ada lirik tersimpan yang bisa dibaca. Gunakan Cari ulang atau Muat file .lrc.".into()
        } else {
            "Pilih rekaman yang sama pada daftar Lirik yang dipakai. Jika belum terlihat, perbaiki kolom Judul / Artis lalu klik Lirik tersimpan lagi.".into()
        };
        let labels: Vec<String> = self.choices.iter().map(|c| c.label.clone()).collect();
        window.set_choices(&labels, None);
        self.refresh_lyrics_window();
    }

    fn lyrics_action(&mut self, action: lyrics_window::Action) {
        use lyrics_window::Action;
        match action {
            Action::Search => self.search_choices(),
            Action::Choose(index) => {
                self.window_note.clear();
                self.choose(index);
            }
            Action::Mode(index) => {
                self.lyrics_mode = lyrics::Mode::from_index(index);
                write_setting("LyricsMode", index);
                // Pilihan yang sedang dipakai diterapkan ulang dengan jenis lirik baru.
                match self.chosen {
                    Some(chosen) => self.choose(chosen),
                    None => {
                        let media = self.media.lock().unwrap().clone();
                        if media.active {
                            self.request_lyrics(&media, true);
                        }
                    }
                }
                if self.lyrics_mode != lyrics::Mode::Original {
                    self.window_note = "Romaji memakai penganalisis Jepang Windows; bila tidak tersedia, teks asli yang tampil.".into();
                }
                self.refresh_lyrics_window();
            }
            Action::Earlier => self.shift_offset(Some(-0.5)),
            Action::Later => self.shift_offset(Some(0.5)),
            Action::ResetOffset => self.shift_offset(None),
            Action::Save => self.save_lyrics_file(),
            Action::Load => self.load_lyrics_file(),
            Action::Saved => self.show_saved_lyrics(),
            Action::Closed => {
                self.lyrics_window = None;
            }
        }
    }

    /// Equalizer notch kecil bergerak hanya saat terlihat (lirik diputar, notch tidak terbuka).
    fn update_equalizer(&self, visible: bool) {
        unsafe {
            if visible && self.t < 0.5 && !self.hidden {
                SetTimer(self.hwnd, TIMER_EQUALIZER, 80, None);
            } else {
                let _ = KillTimer(self.hwnd, TIMER_EQUALIZER);
            }
        }
    }

    fn start_animation_timer(&self) {
        unsafe {
            SetTimer(self.hwnd, TIMER_ANIMATION, 15, None);
        }
    }

    fn animate_to(&mut self, target: f32) {
        if self.target == target {
            return;
        }
        self.from = self.t;
        self.target = target;
        self.started = Instant::now();
        if target < 0.5 {
            self.search_focused = false;
        }
        self.start_animation_timer();
        unsafe {
            // Saat terbuka, progres lagu dan kursor teks diperbarui lebih sering.
            SetTimer(self.hwnd, TIMER_TICK, if target > 0.5 { 250 } else { 1000 }, None);
        }
    }

    /// Satu langkah animasi: buka/tutup notch, lebar notch kecil, dan geser halaman.
    fn step_animation(&mut self) {
        let p = (self.started.elapsed().as_secs_f32() / ANIMATION_SECONDS).min(1.0);
        let eased = 1.0 - (1.0 - p).powi(3); // Ease-out cubic.
        self.t = self.from + (self.target - self.from) * eased;
        let opening_done = p >= 1.0;
        if opening_done {
            self.t = self.target;
        }
        let step = |current: f32, target: f32| if (target - current).abs() < 0.5 { target } else { current + (target - current) * 0.3 };
        self.compact = (step(self.compact.0, self.compact_target.0), step(self.compact.1, self.compact_target.1));
        if let Some((_, _, started)) = self.page_transition {
            if started.elapsed().as_secs_f32() >= PAGE_SECONDS {
                self.page_transition = None;
            }
        }
        if opening_done && self.compact == self.compact_target && self.page_transition.is_none() {
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ANIMATION);
            }
        }
        self.render();
    }

    fn navigate_page(&mut self, direction: i32) {
        let count = draw::PAGE_COUNT as i32;
        let from = self.page;
        self.page = ((self.page as i32 + direction).rem_euclid(count)) as usize;
        self.page_transition = Some((from, direction.signum() as f32, Instant::now()));
        self.search_focused = false;
        write_setting("Page", self.page as u32);
        self.start_animation_timer();
        self.render();
    }

    // ---- Clipboard -------------------------------------------------------------------------

    fn clipboard_changed(&mut self) {
        if !self.recording || clipboard::sequence() == self.clipboard_sequence {
            return;
        }
        match clipboard::read(self.hwnd) {
            clipboard::Read::Busy => {
                // Clipboard dipakai aplikasi lain: coba lagi sebentar, tanpa polling saat diam.
                if self.clipboard_retries < 3 {
                    self.clipboard_retries += 1;
                    unsafe {
                        SetTimer(self.hwnd, TIMER_CLIPBOARD_RETRY, 45, None);
                    }
                }
                return;
            }
            clipboard::Read::Skip => {}
            clipboard::Read::TooLarge => self.clipboard_status = "Teks terlalu besar; maksimal 16.384 karakter per salinan.".into(),
            clipboard::Read::Text(text) => {
                let time = unsafe { GetLocalTime() };
                if self.history.add(&text, format!("{:02}:{:02}", time.wHour, time.wMinute)) {
                    self.clipboard_status = "Tersimpan untuk sesi ini · hilang saat aplikasi ditutup".into();
                }
            }
        }
        self.clipboard_retries = 0;
        self.clipboard_sequence = clipboard::sequence();
        self.render();
    }

    fn clipboard_hit(&self) -> (Vec<u64>, usize) {
        let ids = self.history.find(&self.search).iter().map(|item| item.id).collect();
        (ids, self.history.items().len())
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        let (ids, total) = self.clipboard_hit();
        let clip = ClipHit { ids: &ids, total, scroll: self.scroll };
        let page = if self.page_transition.is_some() { usize::MAX } else { self.page };
        draw::hit_test(self.t, page, &self.media.lock().unwrap(), self.gfx.track, &clip, x, y)
    }

    fn scroll_by(&mut self, delta: f32) {
        let rows = self.history.find(&self.search).len() as f32;
        let max = (rows * draw::CLIP_ROW_H - draw::CLIP_LIST_H).max(0.0);
        self.scroll = (self.scroll + delta).clamp(0.0, max);
        self.render();
    }

    /// Ketik di kotak cari clipboard.
    fn on_char(&mut self, ch: u32) {
        if !self.search_focused {
            return;
        }
        match ch {
            8 => {
                self.search.pop();
            }
            27 => self.search_focused = false,
            // Ctrl+V: tempel teks clipboard ke kotak cari (satu baris).
            22 => {
                if let clipboard::Read::Text(text) = clipboard::read(self.hwnd) {
                    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    self.search.extend(line.chars().take(256usize.saturating_sub(self.search.chars().count())));
                }
            }
            c if c >= 32 => {
                if let Some(c) = char::from_u32(c) {
                    if self.search.chars().count() < 256 {
                        self.search.push(c);
                    }
                }
            }
            _ => return,
        }
        self.scroll = 0.0;
        self.render();
    }

    fn to_dip(&self, lparam: LPARAM) -> (f32, f32) {
        let x = (lparam.0 & 0xFFFF) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
        let scale = self.dpi / 96.0;
        (x / scale, y / scale)
    }

    fn on_mouse_move(&mut self, lparam: LPARAM) {
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
            if !self.tracking {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd,
                    dwHoverTime: 0,
                };
                self.tracking = TrackMouseEvent(&mut track).is_ok();
            }
        }
        let (x, y) = self.to_dip(lparam);
        if draw::inside_shape(self.t, self.compact, x, y) {
            self.animate_to(1.0);
        }
        let hover = self.hit(x, y);
        if hover != self.hover {
            self.hover = hover;
            self.render();
        }
    }

    fn on_mouse_leave(&mut self) {
        self.tracking = false;
        if self.hover != Hit::None {
            self.hover = Hit::None;
            self.render();
        }
        unsafe {
            SetTimer(self.hwnd, TIMER_COLLAPSE, COLLAPSE_DELAY_MS, None);
        }
    }

    fn on_click(&mut self, lparam: LPARAM) {
        let (x, y) = self.to_dip(lparam);
        let hit = self.hit(x, y);
        if hit != Hit::ClipSearch && self.search_focused {
            self.search_focused = false;
            self.render();
        }
        let command = match hit {
            Hit::Lyrics => {
                self.open_lyrics_window();
                return;
            }
            Hit::PageLeft | Hit::PageRight => {
                self.navigate_page(if hit == Hit::PageLeft { -1 } else { 1 });
                return;
            }
            Hit::ClipSearch => {
                // Kotak cari butuh fokus keyboard: aktifkan jendela notch sementara.
                self.search_focused = true;
                unsafe {
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = SetFocus(self.hwnd);
                }
                self.render();
                return;
            }
            Hit::ClipPause => {
                self.recording = !self.recording;
                self.clipboard_sequence = clipboard::sequence();
                write_setting("ClipboardRecording", self.recording as u32);
                self.clipboard_status = if self.recording {
                    "Pencatatan dilanjutkan untuk salinan berikutnya."
                } else {
                    "Pencatatan dijeda. Riwayat sesi tetap tersedia."
                }
                .into();
                self.render();
                return;
            }
            Hit::ClipReset => {
                self.history.clear();
                self.search.clear();
                self.scroll = 0.0;
                self.clipboard_sequence = clipboard::sequence();
                self.clipboard_status = "Riwayat notch sudah dikosongkan.".into();
                self.render();
                return;
            }
            Hit::ClipCopy(id) | Hit::ClipRow(id) => {
                let text = self.history.items().iter().find(|item| item.id == id).map(|item| item.text.clone());
                if let Some(text) = text {
                    let copied = (0..3).any(|attempt| {
                        if attempt > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(45));
                        }
                        clipboard::write(self.hwnd, &text)
                    });
                    self.clipboard_sequence = clipboard::sequence();
                    self.clipboard_status = if copied {
                        "Disalin. Tempel ke aplikasi tujuan dengan Ctrl+V."
                    } else {
                        "Clipboard sedang dipakai aplikasi lain. Coba Salin lagi."
                    }
                    .into();
                    self.render();
                }
                return;
            }
            Hit::ClipDelete(id) => {
                if self.history.remove(id) {
                    self.clipboard_sequence = clipboard::sequence();
                    self.clipboard_status = "Salinan dihapus dari riwayat sesi.".into();
                    self.scroll_by(0.0);
                }
                return;
            }
            Hit::Previous => Command::Previous,
            Hit::PlayPause => Command::PlayPause,
            Hit::Next => Command::Next,
            Hit::Seek(fraction) => Command::Seek(fraction as f64),
            Hit::Pin => {
                self.pinned = !self.pinned;
                write_setting("Pinned", self.pinned as u32);
                self.apply_pin();
                if let Some(window) = &self.lyrics_window {
                    window.set_topmost(self.pinned);
                }
                self.render();
                return;
            }
            Hit::Theme => {
                self.theme = (self.theme + 1) % THEMES.len();
                write_setting("Theme", self.theme as u32);
                if let Some(window) = &self.lyrics_window {
                    window.set_theme(&THEMES[self.theme]);
                }
                self.update_tray();
                self.render();
                return;
            }
            Hit::Minimize => {
                self.set_hidden(true);
                return;
            }
            Hit::None => return,
        };
        let _ = self.commands.send(command);
    }

    /// Selalu di atas (pin aktif) atau jendela biasa.
    fn apply_pin(&self) {
        let after = if self.pinned { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Minimalkan ke tray (musik tetap berjalan) atau tampilkan lagi sebagai notch kecil.
    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
            let _ = KillTimer(self.hwnd, TIMER_ANIMATION);
        }
        self.t = 0.0;
        self.from = 0.0;
        self.target = 0.0;
        self.hover = Hit::None;
        unsafe {
            if hidden {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            } else {
                self.render();
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.apply_pin();
            }
        }
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        let tip = format!("NotchRust · Tema: {}", THEMES[self.theme].name);
        for (slot, ch) in data.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *slot = ch;
        }
        data
    }

    fn update_tray(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.tray_data());
        }
    }

    fn add_tray(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.tray_data());
        }
    }

    fn remove_tray(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
        }
    }
}

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_MOUSEMOVE => {
                with_app(|app| app.on_mouse_move(lparam));
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_app(|app| app.on_mouse_leave());
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                with_app(|app| app.on_click(lparam));
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu(hwnd);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_TIMER => {
                match wparam.0 {
                    TIMER_ANIMATION => with_app(|app| app.step_animation()),
                    TIMER_COLLAPSE => with_app(|app| {
                        let _ = KillTimer(hwnd, TIMER_COLLAPSE);
                        app.animate_to(0.0);
                    }),
                    TIMER_CLIPBOARD_RETRY => with_app(|app| {
                        let _ = KillTimer(hwnd, TIMER_CLIPBOARD_RETRY);
                        app.clipboard_changed();
                    }),
                    _ => with_app(|app| app.render()),
                };
                LRESULT(0)
            }
            WM_NET => {
                with_app(|app| app.render());
                LRESULT(0)
            }
            WM_MEDIA | WM_LYRICS => {
                with_app(|app| {
                    app.render();
                    app.refresh_lyrics_window();
                });
                LRESULT(0)
            }
            WM_LYRIC_CHOICES => {
                with_app(|app| app.choices_ready());
                LRESULT(0)
            }
            WM_CLIPBOARDUPDATE => {
                with_app(|app| {
                    app.clipboard_retries = 0;
                    app.clipboard_changed();
                });
                LRESULT(0)
            }
            WM_CHAR => {
                with_app(|app| app.on_char(wparam.0 as u32));
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
                with_app(|app| {
                    if app.t > 0.95 && app.page == draw::PAGE_CLIPBOARD {
                        app.scroll_by(-delta / 120.0 * draw::CLIP_ROW_H);
                    }
                });
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                with_app(|app| {
                    if app.search_focused {
                        app.search_focused = false;
                        app.render();
                    }
                });
                LRESULT(0)
            }
            WM_TRAY => {
                match lparam.0 as u32 {
                    // Klik kiri ikon tray memunculkan lagi notch yang diminimalkan.
                    WM_LBUTTONUP => {
                        let hidden = with_app(|app| app.hidden).unwrap_or(false);
                        if hidden {
                            with_app(|app| app.set_hidden(false));
                        } else {
                            show_menu(hwnd);
                        }
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            // Perintah "Lirik…" tanpa menu klik kanan (pintasan atau aplikasi lain).
            WM_COMMAND if wparam.0 & 0xFFFF == MENU_LYRICS_WINDOW => {
                with_app(|app| app.open_lyrics_window());
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                with_app(|app| {
                    app.layout();
                    app.render();
                });
                LRESULT(0)
            }
            WM_DESTROY => {
                with_app(|app| {
                    if let Some(window) = app.lyrics_window.take() {
                        window.close();
                    }
                    app.remove_tray();
                    if app.clipboard_listening {
                        let _ = RemoveClipboardFormatListener(app.hwnd);
                    }
                    // Riwayat clipboard hanya hidup selama sesi: tidak ada file atau snapshot.
                    app.history.clear();
                });
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                // Explorer dimulai ulang: ikon tray harus didaftarkan lagi.
                with_app(|app| app.add_tray());
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// Menu klik kanan (notch atau tray). Dipanggil tanpa memegang state App, karena menu
/// menjalankan message loop sendiri.
unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let (hidden, offline) = with_app(|app| (app.hidden, app.offline)).unwrap_or((false, false));
    let autostart = autostart_enabled();
    let checked = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    let show_label = if hidden { w!("Tampilkan notch") } else { w!("Minimalkan ke tray") };
    let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW, show_label);
    let _ = AppendMenuW(menu, MF_STRING, MENU_LYRICS_WINDOW, w!("Lirik…"));
    let _ = AppendMenuW(menu, MF_STRING, MENU_REFRESH_LYRICS, w!("Cari ulang lirik"));
    let _ = AppendMenuW(menu, MF_STRING | checked(offline), MENU_OFFLINE, w!("Lirik offline saja"));
    let _ = AppendMenuW(menu, MF_STRING | checked(autostart), MENU_AUTOSTART, w!("Jalan otomatis saat Windows mulai"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Keluar"));
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let _ = SetForegroundWindow(hwnd);
    let chosen: BOOL = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    match chosen.0 as usize {
        MENU_SHOW => {
            with_app(|app| app.set_hidden(!hidden));
        }
        MENU_AUTOSTART => set_autostart(!autostart),
        MENU_OFFLINE => {
            with_app(|app| {
                // Mode offline: tidak ada permintaan ke LRCLIB/Deezer, hanya lirik tersimpan.
                app.offline = !app.offline;
                write_setting("LyricsOffline", app.offline as u32);
                let media = app.media.lock().unwrap().clone();
                if media.active {
                    app.request_lyrics(&media, false);
                }
                app.render();
            });
        }
        MENU_LYRICS_WINDOW => {
            with_app(|app| app.open_lyrics_window());
        }
        MENU_REFRESH_LYRICS => {
            with_app(|app| {
                let media = app.media.lock().unwrap().clone();
                if media.active {
                    app.request_lyrics(&media, true);
                    app.render();
                }
            });
        }
        MENU_EXIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn exe_command() -> Option<Vec<u16>> {
    let exe = std::env::current_exe().ok()?;
    let command = format!("\"{}\"", exe.display());
    Some(command.encode_utf16().chain(std::iter::once(0)).collect())
}

fn autostart_enabled() -> bool {
    let Some(expected) = exe_command() else { return false };
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_NAME,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return false;
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(0);
    let stored = String::from_utf16_lossy(&buffer[..length]);
    let expected = String::from_utf16_lossy(&expected[..expected.len() - 1]);
    stored.eq_ignore_ascii_case(&expected)
}

fn set_autostart(enabled: bool) {
    unsafe {
        if enabled {
            if let Some(command) = exe_command() {
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    RUN_NAME,
                    REG_SZ.0,
                    Some(command.as_ptr() as *const _),
                    (command.len() * 2) as u32,
                );
            }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME);
        }
    }
}

/// Aksi dari jendela lirik (tombol, daftar, tutup).
fn on_lyrics_action(action: lyrics_window::Action) {
    with_app(|app| app.lyrics_action(action));
}

/// Dialog simpan/buka file Windows. `filter` berisi pasangan "nama\0pola\0".
fn file_dialog(owner: Option<HWND>, filter: &str, extension: &str, name: &str, save: bool) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };
    let filter: Vec<u16> = filter.encode_utf16().chain([0, 0]).collect();
    let extension: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buffer = vec![0u16; 1024];
    for (slot, ch) in buffer.iter_mut().zip(name.encode_utf16().take(1000)) {
        *slot = ch;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.unwrap_or_default(),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: PCWSTR(extension.as_ptr()),
        Flags: OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST },
        ..Default::default()
    };
    let ok = unsafe { if save { GetSaveFileNameW(&mut dialog) } else { GetOpenFileNameW(&mut dialog) } };
    if !ok.as_bool() {
        return None;
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn read_setting(name: &str) -> Option<u32> {
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(value)
}

fn write_setting(name: &str, value: u32) {
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            PCWSTR(name.as_ptr()),
            REG_DWORD.0,
            Some(&value as *const u32 as *const _),
            4,
        );
    }
}

/// Pertama kali jalan: pakai tema yang sedang dipakai NotchMusic (ui-settings.json), bila ada.
fn notchmusic_theme() -> Option<usize> {
    let path = std::path::Path::new(&std::env::var_os("LOCALAPPDATA")?).join("NotchMusic").join("ui-settings.json");
    let text = std::fs::read_to_string(path).ok()?;
    let rest = &text[text.find("\"Theme\":")? + 8..];
    let digits: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|&theme: &usize| theme < THEMES.len())
}

/// Ikon tray dibaca dari file di samping exe, sama seperti versi WPF.
fn load_tray_icon() -> HICON {
    unsafe {
        let path = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("NotchMusic_Tray_Mint.ico")));
        if let Some(path) = path {
            let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
            let size = GetSystemMetrics(SM_CXSMICON);
            if let Ok(handle) = LoadImageW(None, PCWSTR(wide.as_ptr()), IMAGE_ICON, size, size, LR_LOADFROMFILE) {
                return HICON(handle.0);
            }
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

/// Waktu sekarang dalam tick 100 ns sejak 1601, sama dengan DateTime WinRT.
fn now_ticks() -> i64 {
    let time = unsafe { GetSystemTimeAsFileTime() };
    (((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64) as i64
}
