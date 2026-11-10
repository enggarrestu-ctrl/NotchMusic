// Membaca lagu yang sedang diputar lewat Windows Media Session (GSMTC): Spotify, YouTube Music,
// YouTube di browser, dan pemutar lain yang terhubung ke Windows. Semua panggilan WinRT berjalan
// di thread ini; UI hanya menerima salinan state lewat PostMessage.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::core::Result;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
// Browser sering mengirim cover beberapa detik setelah judul: coba lagi selama ~5 detik.
const COVER_RETRIES: u32 = 10;

#[derive(Clone, Default)]
pub struct MediaState {
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    pub playing: bool,
    /// Semua waktu dalam tick 100 ns, sama dengan TimeSpan/DateTime WinRT.
    pub start: i64,
    pub end: i64,
    pub position: i64,
    pub updated: i64,
    pub cover: Option<Arc<Vec<u8>>>,
    pub cover_id: u64,
    /// Pemutar mendukung tombol Stop.
    pub can_stop: bool,
    /// Semua sesi media Windows (id AUMID, nama ramah) dan sesi pilihan manual (None = otomatis).
    pub sessions: Vec<(String, String)>,
    pub manual: Option<String>,
}

impl MediaState {
    pub fn duration_secs(&self) -> f64 {
        (self.end - self.start).max(0) as f64 / 1e7
    }

    /// Posisi saat ini: posisi terakhir yang dilaporkan + waktu yang lewat sejak laporan itu.
    pub fn elapsed_secs(&self, now: i64) -> f64 {
        let mut position = self.position - self.start;
        if self.playing && self.updated > 0 && now > self.updated {
            position += now - self.updated;
        }
        position.clamp(0, (self.end - self.start).max(0)) as f64 / 1e7
    }

    fn same_as(&self, other: &MediaState) -> bool {
        self.active == other.active
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.source == other.source
            && self.playing == other.playing
            && self.start == other.start
            && self.end == other.end
            && self.position == other.position
            && self.updated == other.updated
            && self.cover_id == other.cover_id
            && self.can_stop == other.can_stop
            && self.sessions == other.sessions
            && self.manual == other.manual
    }
}

pub enum Command {
    PlayPause,
    Next,
    Previous,
    /// Pecahan 0..1 dari durasi lagu.
    Seek(f64),
    Stop,
    /// Ikuti sesi ini (id AUMID), atau None = ikuti pemutar aktif Windows.
    Select(Option<String>),
    /// "Baca ulang media": minta ulang pengelola sesi Windows.
    Reload,
}

/// Sesi pilihan manual bila masih ada, selain itu sesi aktif Windows.
fn current(manager: Option<&Manager>, manual: Option<&String>) -> Option<Session> {
    let manager = manager?;
    if let Some(id) = manual {
        let chosen = manager.GetSessions().ok()?.into_iter().find(|s| s.SourceAppUserModelId().is_ok_and(|x| x.to_string() == *id));
        if chosen.is_some() {
            return chosen;
        }
    }
    manager.GetCurrentSession().ok()
}

pub fn spawn(hwnd: isize, msg: u32, shared: Arc<Mutex<MediaState>>) -> Sender<Command> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let mut manager: Option<Manager> = None;
        let mut track_key = String::new();
        let mut cover_tries = 0u32;
        let mut next_cover_id = 0u64;
        let mut manual: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Command::Select(id)) => manual = id,
                Ok(Command::Reload) => {
                    manager = None;
                    track_key.clear();
                }
                Ok(command) => {
                    if let Some(session) = current(manager.as_ref(), manual.as_ref()) {
                        let _ = execute(&session, command);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if manager.is_none() {
                manager = Manager::RequestAsync().and_then(|op| op.get()).ok();
            }
            let previous = shared.lock().unwrap().clone();
            let mut state = match current(manager.as_ref(), manual.as_ref()) {
                Some(session) => read_session(
                    &session,
                    &previous,
                    &mut track_key,
                    &mut cover_tries,
                    &mut next_cover_id,
                ),
                None => {
                    track_key.clear();
                    MediaState::default()
                }
            };
            state.manual = manual.clone();
            if let Some(sessions) = manager.as_ref().and_then(|m| m.GetSessions().ok()) {
                state.sessions = sessions
                    .into_iter()
                    .filter_map(|s| s.SourceAppUserModelId().ok().map(|id| id.to_string()))
                    .map(|id| (id.clone(), friendly_source(&id)))
                    .collect();
            }
            if !state.same_as(&previous) {
                *shared.lock().unwrap() = state;
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
    tx
}

fn read_session(
    session: &Session,
    previous: &MediaState,
    track_key: &mut String,
    cover_tries: &mut u32,
    next_cover_id: &mut u64,
) -> MediaState {
    let mut state = MediaState {
        active: true,
        source: session
            .SourceAppUserModelId()
            .map(|id| friendly_source(&id.to_string()))
            .unwrap_or_default(),
        ..Default::default()
    };

    if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
        state.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        state.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        state.album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
        let key = format!("{}\u{1}{}\u{1}{}", state.source, state.title, state.artist);
        if key != *track_key {
            *track_key = key;
            *cover_tries = 0;
            state.cover = None;
            *next_cover_id += 1;
            state.cover_id = *next_cover_id;
        } else {
            state.cover = previous.cover.clone();
            state.cover_id = previous.cover_id;
        }
        if state.cover.is_none() && *cover_tries < COVER_RETRIES {
            *cover_tries += 1;
            if let Some(bytes) = read_cover(&props) {
                state.cover = Some(Arc::new(bytes));
                *next_cover_id += 1;
                state.cover_id = *next_cover_id;
            }
        }
    }

    if let Ok(info) = session.GetPlaybackInfo() {
        state.playing = info.PlaybackStatus().map(|s| s == Status::Playing).unwrap_or(false);
        state.can_stop = info.Controls().and_then(|c| c.IsStopEnabled()).unwrap_or(false);
    }
    if let Ok(timeline) = session.GetTimelineProperties() {
        state.start = timeline.StartTime().map(|t| t.Duration).unwrap_or(0);
        state.end = timeline.EndTime().map(|t| t.Duration).unwrap_or(0);
        state.position = timeline.Position().map(|t| t.Duration).unwrap_or(0);
        state.updated = timeline.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
    }
    state
}

fn read_cover(props: &Properties) -> Option<Vec<u8>> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_COVER_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let loaded = reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

fn execute(session: &Session, command: Command) -> Result<bool> {
    match command {
        Command::PlayPause => session.TryTogglePlayPauseAsync()?.get(),
        Command::Next => session.TrySkipNextAsync()?.get(),
        Command::Previous => session.TrySkipPreviousAsync()?.get(),
        Command::Stop => session.TryStopAsync()?.get(),
        Command::Select(_) | Command::Reload => Ok(false), // Ditangani di loop media.
        Command::Seek(fraction) => {
            let timeline = session.GetTimelineProperties()?;
            let start = timeline.StartTime()?.Duration;
            let end = timeline.EndTime()?.Duration;
            let target = start + ((end - start) as f64 * fraction.clamp(0.0, 1.0)) as i64;
            session.TryChangePlaybackPositionAsync(target)?.get()
        }
    }
}

fn friendly_source(id: &str) -> String {
    let value = id.to_lowercase();
    let name = if value.contains("spotify") {
        "Spotify"
    } else if value.contains("msedge") {
        "Microsoft Edge"
    } else if value.contains("chrome") {
        "Google Chrome"
    } else if value.contains("brave") {
        "Brave"
    } else if value.contains("firefox") {
        "Firefox"
    } else if value.contains("youtube") {
        "YouTube / YouTube Music"
    } else if id.trim().is_empty() {
        "Pemutar media"
    } else {
        return id.trim_end_matches(".exe").to_string();
    };
    name.to_string()
}
