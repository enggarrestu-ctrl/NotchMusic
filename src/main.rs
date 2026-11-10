// NotchRust: widget notch di atas layar Windows, port Rust dari NotchMusic (WPF).
// Saat diam: logo, jam, kecepatan internet (atau baris lirik saat lagu diputar).
// Saat di-hover: panel musik + lirik, dan halaman riwayat clipboard.
// Tanpa jendela konsol saat jalan; saat `cargo test` tetap konsol agar hasil tes terlihat.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod clipboard;
mod desktop;
mod draw;
mod games;
mod http;
mod lyrics;
mod lyrics_window;
mod media;
mod menu;
mod motion;
mod net;
mod notify;
mod prayer;
mod theme_editor;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{
    BOOL, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY};
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
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use draw::{ClipHit, ClipRow, ClipView, GameView, Gfx, Hit, LyricView, PrayerView, View, COMPACT_H, COMPACT_W, EXPANDED_H, EXPANDED_W, THEMES};
use media::{Command, MediaState};
use net::Speed;

const WM_MEDIA: u32 = WM_APP + 1;
const WM_NET: u32 = WM_APP + 2;
const WM_TRAY: u32 = WM_APP + 3;
const WM_LYRICS: u32 = WM_APP + 4;
const WM_LYRIC_CHOICES: u32 = WM_APP + 5;
const WM_NOTICE: u32 = WM_APP + 7;
const WM_ADD_GAME: u32 = WM_APP + 8;
const GAME_HINT: &str = "Klik buka · seret untuk pindah · + tambah · klik kanan hapus";
const NOTICE_SECONDS: f32 = 5.0;
// Tidak tersedia di modul WindowsAndMessaging crate windows.
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;

// 1 dulu TIMER_ANIMATION; animasi kini digerakkan vsync di loop pesan.
const TIMER_TICK: usize = 2;
const TIMER_COLLAPSE: usize = 3;
const TIMER_EQUALIZER: usize = 4;
const TIMER_CLIPBOARD_RETRY: usize = 5;



const COLLAPSE_DELAY_MS: u32 = 350;

const MENU_AUTOSTART: usize = 1;
const MENU_EXIT: usize = 2;
const MENU_SHOW: usize = 3;
const MENU_OFFLINE: usize = 4;
const MENU_REFRESH_LYRICS: usize = 5;
const MENU_LYRICS_WINDOW: usize = 6;
const MENU_CLIPBOARD: usize = 7;
const MENU_PRAYER: usize = 8;
const MENU_SPEED: usize = 9;
const MENU_RESET_POSITION: usize = 10;
const MENU_GAMES: usize = 11;
const MENU_EDIT_THEME: usize = 12;

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
    /// Ada animasi berjalan: loop pesan menggambar satu frame per vsync.
    animating: bool,
    /// Waktu langkah animasi terakhir.
    stepped: Instant,
    /// Pantulan pegas saat membuka (lihat draw::View::bounce).
    bounce: f32,
    /// Kota jadwal salat (indeks prayer::CITIES).
    prayer_city: usize,
    /// Menit terakhir yang sudah dicek untuk pengingat salat.
    prayer_minute: i64,
    /// Pengingat singkat di notch kecil dan kapan mulai tampil.
    notice: Option<(String, Instant)>,
    /// Daftar kota terbuka di halaman salat: posisi gulir.
    city_picker: Option<f32>,
    /// Halaman game: kunci tersimpan, game lengkap, status, dan seret ikon (asal, penanda tempat taruh).
    game_keys: Vec<String>,
    games: Vec<games::Game>,
    game_status: String,
    games_enabled: bool,
    game_press: Option<(usize, POINT)>,
    game_drag: Option<(usize, Option<(usize, bool)>)>,
    /// Fitur yang bisa dimatikan dari menu klik kanan.
    clipboard_enabled: bool,
    prayer_enabled: bool,
    /// "Notifikasi dan kecepatan internet".
    speed_enabled: bool,
    notices: std::sync::Arc<notify::Shared>,
    /// Kunci game: saat mouse masuk ada game layar penuh -> notch tidak terbuka/menu tidak muncul,
    /// klik kiri dua kali membukanya. Dicatat saat masuk karena notch bisa jadi jendela depan setelah diklik.
    game_latched: bool,
    game_unlocked: bool,
    /// Posisi notch: menempel di atas monitor atau mengambang; tengah notch (piksel layar) dan atas jendela.
    docked: bool,
    position: Option<(i32, i32)>,
    /// Tombol kiri ditekan di area kosong (posisi kursor layar); diseret melewati ambang = pindah notch.
    pressed: Option<POINT>,
    /// Sedang diseret: jarak kursor ke kiri-atas notch kecil (piksel).
    grab: Option<(i32, i32)>,
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
    /// Tema kustom tersimpan (urutan terakhir setelah tema bawaan) dan pratinjau editor tema.
    custom_theme: Option<draw::Theme>,
    preview_theme: Option<draw::Theme>,
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
            style: CS_DBLCLKS, // Klik kiri dua kali membuka kunci game.
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("NotchRustWindow"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        // Selalu di atas, tidak muncul di Alt+Tab/taskbar, dan tidak mengambil fokus saat diklik.
        // NOREDIRECTIONBITMAP: isi jendela sepenuhnya dari DirectComposition (lihat draw.rs).
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
        let speed_enabled = read_setting("SpeedEnabled").map_or(true, |v| v != 0);
        let notices = notify::spawn(hwnd.0 as isize, WM_NOTICE, speed_enabled);
        notify::watch_pads(&notices, hwnd.0 as isize, WM_NOTICE);

        let app = App {
            hwnd,
            gfx: Gfx::new()?,
            dpi: 96.0,
            origin: POINT::default(),
            t: 0.0,
            animating: false,
            stepped: Instant::now(),
            bounce: 0.0,
            prayer_city: read_setting("PrayerCity").map(|v| v as usize).filter(|&i| i < prayer::CITIES.len()).unwrap_or(prayer::JAKARTA),
            prayer_minute: -1,
            notice: None,
            city_picker: None,
            game_keys: games::load_keys(),
            games: Vec::new(),
            game_status: GAME_HINT.into(),
            games_enabled: read_setting("GamesEnabled").map_or(true, |v| v != 0),
            game_press: None,
            game_drag: None,
            clipboard_enabled: read_setting("ClipboardEnabled").map_or(true, |v| v != 0),
            prayer_enabled: read_setting("PrayerEnabled").map_or(true, |v| v != 0),
            speed_enabled,
            notices,
            game_latched: false,
            game_unlocked: false,
            docked: read_setting("Docked").map_or(true, |v| v != 0),
            position: read_setting("CenterX").zip(read_setting("TopY")).map(|(x, y)| (x as i32, y as i32)),
            pressed: None,
            grab: None,
            from: 0.0,
            target: 0.0,
            started: Instant::now(),
            tracking: false,
            hover: Hit::None,
            media,
            speed,
            commands,
            tray_icon: load_tray_icon(),
            theme: read_setting("Theme").map(|v| v as usize).or_else(notchmusic_theme).unwrap_or(DEFAULT_THEME) % (THEMES.len() + 1),
            custom_theme: theme_editor::load().map(theme_editor::to_theme),
            preview_theme: None,
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
            app.reload_games();
            app.apply_features(); // Halaman tersimpan yang fiturnya mati kembali ke Musik; lalu gambar.
            app.add_tray();
            app.apply_pin();
            app.clipboard_listening = AddClipboardFormatListener(app.hwnd).is_ok();
            if !app.clipboard_listening {
                app.clipboard_status = "Pencatatan belum aktif. Tutup dan buka aplikasi lagi.".into();
            }
        });
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_TICK, 1000, None);

        // Saat ada animasi: proses semua pesan yang menunggu, lalu satu frame (Present menunggu vsync),
        // jadi animasi berjalan tepat di refresh monitor dan input tetap responsif. Saat diam: tidur.
        let mut message = MSG::default();
        'messages: loop {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    break 'messages;
                }
                if theme_editor::dialog_message(&message) {
                    continue;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if with_app(|app| app.animating).unwrap_or(false) {
                let frame = Instant::now();
                with_app(|app| app.step_animation());
                // Pengaman bila Present tidak menunggu (jendela tersembunyi): jangan memutar CPU penuh.
                if frame.elapsed().as_millis() < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(4));
                }
            } else {
                let _ = WaitMessage();
            }
        }
        APP.with(|cell| cell.borrow_mut().take());
        Ok(())
    }
}

impl App {
    /// Letakkan notch di posisi tersimpan (default: tengah atas monitor utama), sesuai DPI monitor tersebut.
    fn layout(&mut self) {
        unsafe {
            let monitor = match self.position {
                Some((x, y)) => MonitorFromPoint(POINT { x, y: y + 1 }, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
            };
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.dpi = dpi_x as f32;
            let scale = self.dpi / 96.0;
            let width = (EXPANDED_W * scale).ceil() as i32;
            let height = ((EXPANDED_H + draw::BOUNCE_ROOM) * scale).ceil() as i32;
            let screen = info.rcMonitor;
            let center = self.position.map_or((screen.left + screen.right) / 2, |p| p.0);
            let top = if self.docked { screen.top } else { self.position.map_or(screen.top, |p| p.1) };
            self.origin = POINT {
                x: (center - width / 2).clamp(screen.left, (screen.right - width).max(screen.left)),
                y: top.clamp(screen.top, (screen.bottom - height).max(screen.top)),
            };
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

        // Notch kecil melebar mengikuti panjang baris lirik (MusicMinWidth + CompactChrome) atau pengingat.
        let compact_line = lines.compact.as_deref();
        let notice = self.notice.as_ref().map(|(text, _)| text.clone());
        self.compact_target = match (&notice, compact_line) {
            (Some(text), _) => ((self.gfx.compact_lyric_width(text) + 32.0 * 2.0 + 6.0).clamp(COMPACT_W, EXPANDED_W), draw::MUSIC_H),
            (None, Some(line)) => (
                (self.gfx.compact_lyric_width(line) + draw::COMPACT_CHROME).clamp(COMPACT_W, EXPANDED_W),
                draw::MUSIC_H,
            ),
            (None, None) => (draw::idle_width(self.speed_enabled), COMPACT_H),
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
            (from, direction, (started.elapsed().as_secs_f32() / motion::PAGE_SECONDS).min(1.0))
        });
        let city = &prayer::CITIES[self.prayer_city];
        let (day, minute_now) = city.now();
        let today = prayer::times(day, city);
        let (next_index, next_time, left) = prayer::next(&today, prayer::times(day + 1, city)[0], minute_now);
        let countdown = prayer::countdown(left);
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
            theme: &self.theme(),
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
            prayer: PrayerView {
                city: city.name,
                zone: city.zone(),
                times: today,
                now: minute_now,
                next: next_index,
                next_time,
                countdown: &countdown,
                city_index: self.prayer_city,
                picker: self.city_picker,
            },
            games: GameView {
                tiles: self.games.iter().collect(),
                status: &self.game_status,
                drop: self.game_drag.and_then(|(_, drop)| drop),
                dragging: self.game_drag.map(|(from, _)| from),
            },
            notice: notice.as_deref(),
            arrows: self.pages().len() > 1,
            bounce: self.bounce,
            docked: self.docked,
            speed: self.speed_enabled,
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
        let theme = &self.theme();
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

    fn start_animation_timer(&mut self) {
        if !self.animating {
            self.stepped = Instant::now();
        }
        self.animating = true;
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
            self.city_picker = None;
        }
        self.start_animation_timer();
        unsafe {
            // Saat terbuka, progres lagu dan kursor teks diperbarui lebih sering.
            SetTimer(self.hwnd, TIMER_TICK, if target > 0.5 { 250 } else { 1000 }, None);
        }
    }

    /// Satu langkah animasi: buka/tutup notch, lebar notch kecil, dan geser halaman.
    fn step_animation(&mut self) {
        // Membuka: pegas dengan pantulan kecil ke bawah. Menutup: lebih cepat, tanpa pantulan.
        let opening = self.target > self.from;
        let seconds = if opening { motion::OPEN_SECONDS } else { motion::CLOSE_SECONDS };
        let p = (self.started.elapsed().as_secs_f32() / seconds).min(1.0);
        let eased = motion::spring(p, 1.0);
        self.t = self.from + (self.target - self.from) * eased;
        self.bounce = if opening { (motion::spring(p, motion::OPEN_DAMPING) - eased) * (self.target - self.from) } else { 0.0 };
        let opening_done = p >= 1.0;
        if opening_done {
            self.t = self.target;
            self.bounce = 0.0;
        }
        // Lebar notch kecil mendekati target 30% per 15 ms, dihitung dari waktu nyata (frame vsync tidak tetap).
        let dt = self.stepped.elapsed().as_secs_f32().min(0.1);
        self.stepped = Instant::now();
        let k = 1.0 - 0.7f32.powf(dt / 0.015);
        let step = |current: f32, target: f32| if (target - current).abs() < 0.5 { target } else { current + (target - current) * k };
        self.compact = (step(self.compact.0, self.compact_target.0), step(self.compact.1, self.compact_target.1));
        if let Some((_, _, started)) = self.page_transition {
            if started.elapsed().as_secs_f32() >= motion::PAGE_SECONDS {
                self.page_transition = None;
            }
        }
        if opening_done && self.compact == self.compact_target && self.page_transition.is_none() {
            self.animating = false;
        }
        self.render();
    }

    /// Tiap detik: tutup pengingat setelah beberapa detik, dan tampilkan pengingat saat masuk waktu salat.
    fn tick(&mut self) {
        if self.notice.as_ref().is_some_and(|(_, shown)| shown.elapsed().as_secs_f32() >= NOTICE_SECONDS) {
            self.notice = None;
        }
        if !self.prayer_enabled {
            return;
        }
        let city = &prayer::CITIES[self.prayer_city];
        let (day, now) = city.now();
        let minute = day * 1440 + now.floor() as i64;
        if minute == self.prayer_minute {
            return;
        }
        let first = self.prayer_minute < 0;
        self.prayer_minute = minute;
        if first {
            return; // Saat aplikasi baru jalan, jangan mengumumkan waktu yang sedang berlangsung.
        }
        if let Some(due) = prayer::times(day, city).iter().find(|p| p.prays && p.minutes.floor() as i64 == now.floor() as i64) {
            self.notice = Some((format!("Waktu {} · {} {}", due.name, due.label(), city.zone()), Instant::now()));
        }
    }

    /// Halaman yang aktif, urut: musik, clipboard, jadwal salat.
    fn pages(&self) -> Vec<usize> {
        let mut pages = vec![draw::PAGE_MUSIC];
        if self.clipboard_enabled {
            pages.push(draw::PAGE_CLIPBOARD);
        }
        if self.games_enabled {
            pages.push(draw::PAGE_GAME);
        }
        if self.prayer_enabled {
            pages.push(draw::PAGE_PRAYER);
        }
        pages
    }

    /// Setelah fitur dinyalakan/dimatikan: halaman yang hilang kembali ke Musik.
    fn apply_features(&mut self) {
        if !self.pages().contains(&self.page) {
            self.page = draw::PAGE_MUSIC;
            self.page_transition = None;
            write_setting("Page", self.page as u32);
        }
        if !self.clipboard_enabled {
            // Riwayat dibuang saat fitur dimatikan; isi clipboard saat ini tidak diimpor saat dinyalakan lagi.
            self.history.clear();
            self.search.clear();
            self.search_focused = false;
            self.scroll = 0.0;
        }
        self.clipboard_sequence = clipboard::sequence();
        self.notices.enabled.store(self.speed_enabled, std::sync::atomic::Ordering::Relaxed);
        if !self.speed_enabled && self.notice.as_ref().is_some_and(|(text, _)| !text.starts_with("Waktu ")) {
            self.notice = None;
        }
        if !self.prayer_enabled && self.notice.as_ref().is_some_and(|(text, _)| text.starts_with("Waktu ")) {
            self.notice = None;
        }
        self.render();
    }

    fn navigate_page(&mut self, direction: i32) {
        let pages = self.pages();
        if pages.len() < 2 {
            return;
        }
        let from = self.page;
        let index = pages.iter().position(|&p| p == from).map_or(0, |i| (i as i32 + direction).rem_euclid(pages.len() as i32) as usize);
        self.page = pages[index];
        self.page_transition = Some((from, direction.signum() as f32, Instant::now()));
        self.search_focused = false;
        write_setting("Page", self.page as u32);
        self.start_animation_timer();
        self.render();
    }

    // ---- Clipboard -------------------------------------------------------------------------

    fn clipboard_changed(&mut self) {
        if !self.clipboard_enabled || !self.recording || clipboard::sequence() == self.clipboard_sequence {
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
        draw::hit_test(self.t, page, &self.media.lock().unwrap(), self.gfx.track, &clip, self.city_picker, self.games.len(), x, y)
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

    fn game_locked(&self) -> bool {
        self.game_latched && !self.game_unlocked
    }

    /// Klik kiri dua kali saat terkunci: notch terbuka dan bisa dipakai sampai menutup lagi.
    fn unlock_game(&mut self) {
        if self.game_locked() {
            self.game_unlocked = true;
            self.animate_to(1.0);
        }
    }

    // ---- Memindah notch -------------------------------------------------------------------

    /// Tombol kiri di area kosong (bukan tombol/slider) bisa jadi awal menyeret notch;
    /// di ikon game bisa jadi awal menyeret ikon untuk mengubah urutan.
    fn on_press(&mut self, lparam: LPARAM) {
        let (x, y) = self.to_dip(lparam);
        if self.game_locked() {
            return;
        }
        if let Hit::GameTile(i) = self.hit(x, y) {
            let mut cursor = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut cursor);
                SetCapture(self.hwnd);
            }
            self.game_press = Some((i, cursor));
            return;
        }
        if self.hit(x, y) != Hit::None {
            return;
        }
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
            SetCapture(self.hwnd);
        }
        self.pressed = Some(cursor);
    }

    /// Kembalikan true bila gerakan ini dipakai untuk menyeret (bukan hover biasa).
    fn drag_to(&mut self) -> bool {
        let Some(pressed) = self.pressed else { return false };
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let scale = self.dpi / 96.0;
        if self.grab.is_none() {
            let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if (cursor.x - pressed.x).abs() < dx && (cursor.y - pressed.y).abs() < dy {
                return true;
            }
            // Mulai menyeret: notch langsung kembali kecil, titik yang dipegang tetap di bawah kursor.
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
            }
            self.t = 0.0;
            self.from = 0.0;
            self.target = 0.0;
            self.bounce = 0.0;
            self.search_focused = false;
            let (x0, w, h) = draw::shape_bounds(0.0, self.compact);
            let left = self.origin.x + (x0 * scale) as i32;
            let grab_x = (cursor.x - left).clamp(0, (w * scale) as i32);
            let grab_y = (cursor.y - self.origin.y).clamp(0, (h * scale) as i32);
            self.grab = Some((grab_x, grab_y));
        }
        let (grab_x, grab_y) = self.grab.unwrap();
        let (x0, _, _) = draw::shape_bounds(0.0, self.compact);
        unsafe {
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let screen = info.rcMonitor;
            let width = (EXPANDED_W * scale).ceil() as i32;
            let top = cursor.y - grab_y;
            // Menempel bila dekat tepi atas; ambang lebih besar saat sudah menempel agar tidak mudah lepas.
            self.docked = top <= screen.top + ((if self.docked { 12.0 } else { 6.0 }) * scale) as i32;
            let left = cursor.x - grab_x - (x0 * scale) as i32;
            self.origin = POINT {
                x: left.clamp(screen.left, (screen.right - width).max(screen.left)),
                y: if self.docked { screen.top } else { top.clamp(screen.top, screen.bottom - (EXPANDED_H * scale) as i32) },
            };
        }
        self.render();
        true
    }

    /// Lepas tombol: bila tadi menyeret, simpan posisi dan susun ulang (DPI monitor baru); kembalikan true.
    fn finish_drag(&mut self) -> bool {
        self.pressed = None;
        let dragged = self.grab.take().is_some();
        unsafe {
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
        }
        if dragged {
            let width = (EXPANDED_W * self.dpi / 96.0).ceil() as i32;
            let position = (self.origin.x + width / 2, self.origin.y);
            self.position = Some(position);
            write_setting("Docked", self.docked as u32);
            write_setting("CenterX", position.0 as u32);
            write_setting("TopY", position.1 as u32);
            self.layout();
            self.render();
        }
        dragged
    }

    fn reset_position(&mut self) {
        self.position = None;
        self.docked = true;
        write_setting("Docked", 1);
        delete_setting("CenterX");
        delete_setting("TopY");
        self.layout();
        self.render();
    }

    /// Seret ikon game: penanda di kiri/kanan kotak tujuan. Kembalikan true bila sedang menyeret ikon.
    fn drag_game(&mut self, lparam: LPARAM) -> bool {
        let Some((from, pressed)) = self.game_press else { return false };
        if self.game_drag.is_none() {
            let mut cursor = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut cursor);
            }
            let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if (cursor.x - pressed.x).abs() < dx && (cursor.y - pressed.y).abs() < dy {
                return true;
            }
        }
        let (x, y) = self.to_dip(lparam);
        let count = self.games.len();
        // Pindah ke kanan = ditaruh sesudah kotak tujuan; ke kiri = sebelum. Kotak "+" = paling belakang (kirinya).
        let drop = draw::game_tile_at(count, x, y).and_then(|tile| {
            let to = tile.min(count.saturating_sub(1));
            (to != from).then_some((tile, from < to && tile < count))
        });
        self.game_drag = Some((from, drop));
        self.render();
        true
    }

    /// Lepas tombol setelah menyeret ikon: susun ulang dan simpan. Kembalikan true bila tadi menyeret.
    fn finish_game_drag(&mut self) -> bool {
        let pressed = self.game_press.take().is_some();
        let Some((from, drop)) = self.game_drag.take() else {
            if pressed {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            return false;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some((tile, _)) = drop {
            if games::reorder(&mut self.game_keys, from, tile) {
                games::save_keys(&self.game_keys);
                self.games = games::resolve(&self.game_keys);
            }
        }
        self.render();
        true
    }

    fn reload_games(&mut self) {
        self.games = games::resolve(&self.game_keys);
        // Game yang di-uninstall ikut keluar dari daftar tersimpan, agar slotnya bisa dipakai lagi.
        if self.games.len() < self.game_keys.len() && !self.games.is_empty() {
            self.game_keys = self.games.iter().map(|g| g.key.clone()).collect();
            games::save_keys(&self.game_keys);
        }
    }

    /// Isi slot kosong dengan game Steam yang terakhir dimainkan dan belum ada di daftar.
    fn scan_steam(&mut self) {
        let free = games::SLOTS.saturating_sub(self.game_keys.len());
        if free == 0 {
            self.game_status = "Daftar penuh. Klik kanan game untuk menghapusnya.".into();
            return;
        }
        let recent = games::steam_games();
        let added: Vec<String> = recent.iter().map(|g| g.id.clone()).filter(|id| !self.game_keys.contains(id)).take(free).collect();
        self.game_status = if added.is_empty() {
            if recent.is_empty() { "Steam atau game Steam tidak ditemukan." } else { "Semua game Steam terbaru sudah ada." }.into()
        } else {
            format!("{} game Steam ditambahkan.", added.len())
        };
        self.game_keys.extend(added);
        games::save_keys(&self.game_keys);
        self.reload_games();
    }

    fn add_game(&mut self, path: String) {
        if self.game_keys.iter().any(|k| k.eq_ignore_ascii_case(&path)) {
            self.game_status = "Game ini sudah ada di daftar.".into();
        } else if self.game_keys.len() >= games::SLOTS {
            self.game_status = "Daftar penuh. Klik kanan game untuk menghapusnya.".into();
        } else {
            let name = std::path::Path::new(&path).file_stem().map_or_else(String::new, |s| s.to_string_lossy().to_string());
            self.game_keys.push(path); // Game baru di belakang; urutan yang sudah diatur tetap.
            games::save_keys(&self.game_keys);
            self.reload_games();
            self.game_status = format!("{name} ditambahkan.");
        }
        self.render();
    }

    fn remove_game(&mut self, index: usize) {
        if index < self.games.len() {
            let key = self.games[index].key.clone();
            self.game_keys.retain(|k| !k.eq_ignore_ascii_case(&key));
            games::save_keys(&self.game_keys);
            self.reload_games();
            self.render();
        }
    }

    fn on_mouse_move(&mut self, lparam: LPARAM) {
        if self.drag_game(lparam) || self.drag_to() {
            return;
        }
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
            if !self.tracking {
                self.game_latched = desktop::fullscreen_app_on_monitor(self.hwnd);
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
        if self.game_locked() {
            return; // Terkunci saat game layar penuh: tidak terbuka, tidak ada efek hover.
        }
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
        if self.city_picker.is_some() && !matches!(hit, Hit::CityItem(_) | Hit::PrayerCity) {
            // Klik di luar daftar kota: tutup daftarnya saja, seperti dropdown.
            self.city_picker = None;
            self.render();
            return;
        }
        if hit != Hit::ClipSearch && self.search_focused {
            self.search_focused = false;
            self.render();
        }
        let command = match hit {
            Hit::Lyrics => {
                self.open_lyrics_window();
                return;
            }
            Hit::PrayerCity => {
                // Buka/tutup daftar kota; saat dibuka, kota terpilih langsung terlihat.
                self.city_picker = match self.city_picker {
                    Some(_) => None,
                    None => Some((self.prayer_city as f32 * draw::PICKER_ROW_H - draw::PICKER_ROW_H).clamp(0.0, draw::PICKER_MAX_SCROLL)),
                };
                self.render();
                return;
            }
            Hit::GameScan => {
                self.scan_steam();
                self.render();
                return;
            }
            Hit::GameAdd => {
                // Dialog file punya loop pesan sendiri: dibuka setelah state App dilepas.
                unsafe {
                    let _ = PostMessageW(self.hwnd, WM_ADD_GAME, WPARAM(0), LPARAM(0));
                }
                return;
            }
            Hit::GameTile(i) => {
                if let Some(game) = self.games.get(i) {
                    self.game_status = if games::launch(game) { format!("Membuka {}…", game.name) } else { format!("{} belum bisa dibuka.", game.name) };
                    self.render();
                }
                return;
            }
            Hit::CityItem(index) => {
                self.prayer_city = index;
                self.city_picker = None;
                write_setting("PrayerCity", index as u32);
                self.render();
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
                let count = THEMES.len() + self.custom_theme.is_some() as usize;
                self.theme = (self.theme % count + 1) % count;
                write_setting("Theme", self.theme as u32);
                self.apply_theme();
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
        }
        self.animating = false;
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
        let tip = format!("NotchRust · Tema: {}", self.theme().name);
        for (slot, ch) in data.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *slot = ch;
        }
        data
    }

    /// Tema aktif: pratinjau editor, lalu tema kustom (indeks terakhir), lalu tema bawaan.
    fn theme(&self) -> draw::Theme {
        match (self.preview_theme, self.custom_theme) {
            (Some(preview), _) => preview,
            (None, Some(custom)) if self.theme == THEMES.len() => custom,
            _ => THEMES[self.theme % THEMES.len()],
        }
    }

    fn apply_theme(&mut self) {
        if let Some(window) = &self.lyrics_window {
            window.set_theme(&self.theme());
        }
        self.update_tray();
        self.render();
    }

    fn theme_action(&mut self, action: theme_editor::Action) {
        match action {
            theme_editor::Action::Preview(colors) => self.preview_theme = Some(theme_editor::to_theme(colors)),
            theme_editor::Action::Save(colors) => {
                theme_editor::save(colors);
                self.preview_theme = None;
                self.custom_theme = Some(theme_editor::to_theme(colors));
                self.theme = THEMES.len();
                write_setting("Theme", self.theme as u32);
            }
            theme_editor::Action::Cancel => self.preview_theme = None,
        }
        self.apply_theme();
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
            WM_LBUTTONDOWN => {
                with_app(|app| app.on_press(lparam));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                with_app(|app| if !app.finish_game_drag() && !app.finish_drag() && !app.game_locked() { app.on_click(lparam) });
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                with_app(|app| app.finish_drag());
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                with_app(|app| app.unlock_game());
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                match with_app(|app| (app.game_locked(), { let (x, y) = app.to_dip(lparam); app.hit(x, y) })) {
                    Some((true, _)) | None => {}
                    Some((false, Hit::GameTile(i))) => show_game_menu(hwnd, i),
                    Some((false, Hit::Theme)) => open_theme_editor(hwnd),
                    Some((false, _)) => show_menu(hwnd),
                }
                LRESULT(0)
            }
            WM_ADD_GAME => {
                if let Some(path) = pick_game_file(hwnd) {
                    with_app(|app| app.add_game(path));
                }
                LRESULT(0)
            }
            // Game tidak kehilangan fokus/ter-minimize saat notch diklik.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MEASUREITEM if menu::measure(hwnd, lparam) => LRESULT(1),
            WM_DRAWITEM if menu::draw(lparam) => LRESULT(1),
            WM_TIMER => {
                match wparam.0 {

                    TIMER_COLLAPSE => with_app(|app| {
                        let _ = KillTimer(hwnd, TIMER_COLLAPSE);
                        app.game_unlocked = false;
                        app.animate_to(0.0);
                    }),
                    TIMER_CLIPBOARD_RETRY => with_app(|app| {
                        let _ = KillTimer(hwnd, TIMER_CLIPBOARD_RETRY);
                        app.clipboard_changed();
                    }),
                    TIMER_TICK => with_app(|app| {
                        app.tick();
                        app.render();
                    }),
                    _ => with_app(|app| app.render()),
                };
                LRESULT(0)
            }
            WM_NOTICE => {
                with_app(|app| {
                    let text = app.notices.pending.lock().unwrap().take();
                    if let Some(text) = text.filter(|_| app.speed_enabled) {
                        app.notice = Some((text, Instant::now()));
                        app.render();
                    }
                });
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
                    } else if let Some(scroll) = app.city_picker {
                        app.city_picker = Some((scroll - delta / 120.0 * draw::PICKER_ROW_H * 2.0).clamp(0.0, draw::PICKER_MAX_SCROLL));
                        app.render();
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

/// Klik kanan ikon game: menu kecil bertema "Hapus dari daftar" (shortcut/game aslinya tidak disentuh).
unsafe fn show_game_menu(hwnd: HWND, index: usize) {
    let Ok(popup) = CreatePopupMenu() else { return };
    let theme = &with_app(|app| app.theme()).unwrap_or(THEMES[0]);
    menu::begin(popup, menu::Colors { background: theme.background, surface: theme.surface, text: 0xFFFFFF });
    menu::append(popup, MF_STRING, 1, "Hapus dari daftar");
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let _ = SetForegroundWindow(hwnd);
    let chosen: BOOL = TrackPopupMenu(popup, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, cursor.x, cursor.y, 0, hwnd, None);
    let _ = DestroyMenu(popup);
    menu::end();
    if chosen.0 == 1 {
        with_app(|app| app.remove_game(index));
    }
}

/// Pilih shortcut (.lnk/.url) atau aplikasi (.exe); shortcut disimpan apa adanya, bukan file tujuannya.
unsafe fn pick_game_file(hwnd: HWND) -> Option<String> {
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NODEREFERENCELINKS, SIGDN_FILESYSPATH};
    let dialog: IFileOpenDialog = windows::Win32::System::Com::CoCreateInstance(&FileOpenDialog, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER).ok()?;
    let options = dialog.GetOptions().ok()?;
    dialog.SetOptions(options | FOS_NODEREFERENCELINKS | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM).ok()?;
    dialog.SetTitle(w!("Pilih shortcut atau aplikasi game")).ok()?;
    dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: w!("Shortcut atau aplikasi"), pszSpec: w!("*.lnk;*.url;*.exe") }]).ok()?;
    dialog.Show(hwnd).ok()?;
    let path = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let text = path.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as _));
    text
}

/// Menu klik kanan (notch atau tray). Dipanggil tanpa memegang state App, karena menu
/// menjalankan message loop sendiri.
unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let (hidden, offline) = with_app(|app| (app.hidden, app.offline)).unwrap_or((false, false));
    let (clipboard_on, prayer_on, speed_on, games_on) =
        with_app(|app| (app.clipboard_enabled, app.prayer_enabled, app.speed_enabled, app.games_enabled)).unwrap_or((true, true, true, true));
    let autostart = autostart_enabled();
    let checked = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    let show_label = if hidden { "Tampilkan notch" } else { "Minimalkan ke tray" };
    // Menu digambar sendiri dengan warna tema notch (lihat menu.rs).
    let theme = &with_app(|app| app.theme()).unwrap_or(THEMES[0]);
    menu::begin(menu, menu::Colors { background: theme.background, surface: theme.surface, text: 0xFFFFFF });
    let item = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| menu::append(menu, flags, id, text);
    item(MF_STRING, MENU_SHOW, show_label);
    item(MF_STRING, MENU_LYRICS_WINDOW, "Lirik…");
    item(MF_STRING, MENU_REFRESH_LYRICS, "Cari ulang lirik");
    item(MF_STRING | checked(offline), MENU_OFFLINE, "Lirik offline saja");
    item(MF_STRING | checked(autostart), MENU_AUTOSTART, "Jalan otomatis saat Windows mulai");
    item(MF_STRING, MENU_EDIT_THEME, "Edit tema…");
    item(MF_STRING, MENU_RESET_POSITION, "Reset posisi ke atas tengah layar");
    menu::separator(menu);
    item(MF_STRING | MF_GRAYED, 0, "Fitur aktif");
    item(MF_STRING | checked(clipboard_on), MENU_CLIPBOARD, "Halaman Clipboard");
    item(MF_STRING | checked(games_on), MENU_GAMES, "Halaman Game");
    item(MF_STRING | checked(prayer_on), MENU_PRAYER, "Halaman Jadwal Salat (dan pengingat)");
    item(MF_STRING | checked(speed_on), MENU_SPEED, "Notifikasi dan kecepatan internet");
    menu::separator(menu);
    item(MF_STRING, MENU_EXIT, "Keluar");    let mut cursor = POINT::default();
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
    menu::end();
    match chosen.0 as usize {
        MENU_SHOW => {
            with_app(|app| app.set_hidden(!hidden));
        }
        MENU_AUTOSTART => set_autostart(!autostart),
        MENU_EDIT_THEME => open_theme_editor(hwnd),
        MENU_RESET_POSITION => {
            with_app(|app| app.reset_position());
        }
        MENU_CLIPBOARD | MENU_PRAYER | MENU_SPEED | MENU_GAMES => {
            with_app(|app| {
                let (name, flag) = match chosen.0 as usize {
                    MENU_CLIPBOARD => ("ClipboardEnabled", &mut app.clipboard_enabled),
                    MENU_GAMES => ("GamesEnabled", &mut app.games_enabled),
                    MENU_PRAYER => ("PrayerEnabled", &mut app.prayer_enabled),
                    _ => ("SpeedEnabled", &mut app.speed_enabled),
                };
                *flag = !*flag;
                write_setting(name, *flag as u32);
                app.apply_features();
            });
        }
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
/// Editor tema (klik kanan tombol tema, atau "Edit tema…" di menu), dimulai dari warna tema aktif.
unsafe fn open_theme_editor(hwnd: HWND) {
    let Some((instance, colors, pinned)) = with_app(|app| (app.instance, theme_editor::colors(&app.theme()), app.pinned)) else { return };
    theme_editor::open(instance, hwnd, colors, pinned, on_theme_action);
}

fn on_theme_action(action: theme_editor::Action) {
    with_app(|app| app.theme_action(action));
}

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

fn delete_setting(name: &str) {
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, PCWSTR(name.as_ptr()));
    }
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
    digits.parse().ok().filter(|&theme: &usize| theme <= THEMES.len()) // Indeks terakhir = tema kustom.
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
