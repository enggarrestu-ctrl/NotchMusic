// Lirik berwaktu: cache lokal, pengenalan lagu dari judul video (Deezer), pencarian LRCLIB,
// pemeringkatan kandidat, dan romaji untuk lagu Jepang (Better Lyrics/Unison, lalu penganalisis fonetik Jepang Windows).
// Port dari LyricsFeature.cs NotchMusic; semua permintaan berjalan di thread pencari ini.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use windows::core::HSTRING;
use windows::Globalization::JapanesePhoneticAnalyzer;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::http;

mod sources;

const MAX_LINES: usize = 1500;
const MAX_SOURCE_CHARS: usize = 200_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub seconds: f64,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Lirik tanpa waktu (bila tidak ada versi berwaktu).
    pub plain: Vec<String>,
    pub instrumental: bool,
    pub source: String,
    /// Koreksi waktu (detik): positif = lirik muncul lebih lambat.
    pub offset: f64,
}

/// Jenis lirik, sama dengan pilihan di jendela lirik NotchMusic.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Mode {
    /// Romaji untuk lagu Jepang, bahasa asli untuk lagu lain.
    #[default]
    Auto,
    Original,
    Romaji,
}

impl Mode {
    pub fn from_index(index: u32) -> Mode {
        match index {
            1 => Mode::Original,
            2 => Mode::Romaji,
            _ => Mode::Auto,
        }
    }

    pub fn index(self) -> u32 {
        match self {
            Mode::Auto => 0,
            Mode::Original => 1,
            Mode::Romaji => 2,
        }
    }
}

/// Satu hasil pencarian untuk daftar "Lirik yang dipakai".
#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub lyrics: Lyrics,
    /// Kandidat yang cukup yakin untuk dipilih otomatis.
    pub automatic: bool,
}

impl Lyrics {
    /// Seluruh teks untuk jendela lirik.
    pub fn text(&self) -> String {
        if self.instrumental && self.lines.is_empty() {
            return "Instrumental — tidak ada lirik.".into();
        }
        if self.is_synced() {
            self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\r\n")
        } else {
            self.plain.join("\r\n")
        }
    }

    /// Ekspor sebagai LRC (berwaktu) atau teks biasa.
    pub fn to_file(&self) -> String {
        if !self.is_synced() {
            return self.plain.join("\r\n");
        }
        self.lines
            .iter()
            .map(|l| {
                let t = (l.seconds + self.offset).max(0.0);
                let centis = (t * 100.0).round() as u64;
                format!("[{:02}:{:02}.{:02}]{}", centis / 6000, centis / 100 % 60, centis % 100, l.text)
            })
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    /// Muat file .lrc (berwaktu) atau .txt (teks biasa).
    pub fn from_file(text: &str) -> Option<Lyrics> {
        let text = text.trim_start_matches('\u{feff}');
        if text.len() > MAX_SOURCE_CHARS {
            return None;
        }
        if let Some(lines) = parse_lrc(text) {
            return Some(Lyrics { lines, source: "File lokal".into(), ..Default::default() });
        }
        let plain: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(MAX_LINES).map(str::to_string).collect();
        (!plain.is_empty()).then_some(Lyrics { plain, source: "File lokal".into(), ..Default::default() })
    }

    pub fn is_synced(&self) -> bool {
        !self.lines.is_empty()
    }

    /// Indeks baris terakhir yang waktunya <= `seconds`.
    pub fn index_at(&self, seconds: f64) -> Option<usize> {
        let count = self.lines.partition_point(|line| line.seconds <= seconds);
        count.checked_sub(1)
    }
}

#[derive(Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Searching,
    Found(Arc<Lyrics>),
    Missing(String),
}

pub struct Request {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub offline: bool,
    /// Lewati cache (menu "Cari ulang lirik").
    pub refresh: bool,
    pub mode: Mode,
}

/// Lagu yang sedang dicari/ditampilkan, plus hasilnya.
#[derive(Clone, Default)]
pub struct Shared {
    pub key: String,
    pub state: State,
}

pub fn spawn(hwnd: isize, msg: u32, shared: Arc<Mutex<Shared>>) -> Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let post = || unsafe {
            let _ = PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0));
        };
        while let Some(request) = latest(&rx) {
            {
                let mut s = shared.lock().unwrap();
                s.key = request.key.clone();
                s.state = State::Searching;
            }
            post();
            let state = resolve(&request);
            let mut s = shared.lock().unwrap();
            // Lagu sudah berganti selama pencarian: hasil lama dibuang.
            if s.key == request.key {
                s.state = state;
                drop(s);
                post();
            }
        }
    });
    tx
}

/// Ambil permintaan terbaru; permintaan lama yang menumpuk dilewati.
fn latest(rx: &Receiver<Request>) -> Option<Request> {
    let mut request = rx.recv().ok()?;
    while let Ok(newer) = rx.try_recv() {
        request = newer;
    }
    Some(request)
}

fn resolve(request: &Request) -> State {
    if request.title.trim().is_empty() {
        return State::Missing("Judul lagu belum tersedia".into());
    }
    let identity = identity(&request.title, &request.artist, &request.album);
    if !request.refresh {
        if let Some(lyrics) = cache_read(&identity) {
            return State::Found(Arc::new(lyrics));
        }
    }
    if request.offline {
        return State::Missing("Lirik offline: lagu ini belum tersimpan".into());
    }
    let choices = search_choices(&request.title, &request.artist, &request.album, request.duration, true, request.mode);
    match choices.map(|c| c.into_iter().find(|c| c.automatic)) {
        Ok(Some(choice)) => {
            let mut lyrics = choice.lyrics;
            apply_mode(&mut lyrics, request.mode);
            // Baris yang tidak terbaca penganalisis Windows: cadangan Unison (gagal = teks asli tetap).
            // shortcut: hanya di pencarian otomatis (thread ini); pilihan manual memakai romaji lokal saja.
            if request.mode != Mode::Original {
                let _ = sources::unison_romanize(&mut lyrics);
            }
            cache_write(&identity, &lyrics);
            State::Found(Arc::new(lyrics))
        }
        Ok(None) => {
            // Cari ulang tanpa hasil: lirik tersimpan yang lama dianggap salah dan dihapus.
            if request.refresh {
                cache_remove(&identity);
            }
            State::Missing("Lirik belum ditemukan otomatis. Buka Aa untuk memilih.".into())
        }
        Err(message) => State::Missing(message),
    }
}

// ---------------------------------------------------------------------------------------------
// Pencarian

#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    track: String,
    artist: String,
    album: String,
    duration: f64,
    instrumental: bool,
    plain: Option<String>,
    synced: Option<String>,
    /// Baris jadi (TTML Better Lyrics/Unison).
    lines: Option<Vec<Line>>,
    /// Sudah romaji (bukan teks Jepang asli).
    romanized: bool,
    source: &'static str,
    language: String,
    id: i64,
}

impl Record {
    fn from_json(value: &Value) -> Option<Record> {
        let text = |name: &str| value.get(name).and_then(Value::as_str).map(|s| s.chars().take(512).collect::<String>());
        let record = Record {
            track: text("trackName").unwrap_or_default(),
            artist: text("artistName").unwrap_or_default(),
            album: text("albumName").unwrap_or_default(),
            duration: value.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
            instrumental: value.get("instrumental").and_then(Value::as_bool).unwrap_or(false),
            plain: value.get("plainLyrics").and_then(Value::as_str).map(str::to_string),
            synced: value.get("syncedLyrics").and_then(Value::as_str).map(str::to_string),
            source: "LRCLIB",
            ..Default::default()
        };
        // Tolak sumber berlebihan; jangan menampilkan potongan lirik seolah lengkap.
        let too_long = |s: &Option<String>| s.as_ref().is_some_and(|s| s.len() > MAX_SOURCE_CHARS);
        (!too_long(&record.plain) && !too_long(&record.synced)).then_some(record)
    }

    fn has_timing(&self) -> bool {
        self.synced.as_ref().is_some_and(|s| !s.trim().is_empty()) || self.lines.as_ref().is_some_and(|l| !l.is_empty())
    }

    /// Sudah romaji, atau judul/album menandainya sebagai romaji.
    fn is_marked_romaji(&self) -> bool {
        static MARKER: OnceLock<Regex> = OnceLock::new();
        self.romanized || re(r"(?i)\b(?:romaji|romanized|romanised|romanization|romanisation)\b", &MARKER).is_match(&format!("{} {}", self.track, self.album))
    }

    /// Rekaman romaji yang bisa dipakai: ditandai romaji, berisi huruf Latin, tanpa aksara Jepang,
    /// dan bukan terjemahan (teks Latin saja bisa terjemahan Inggris).
    fn is_romaji(&self) -> bool {
        static TRANSLATION: OnceLock<Regex> = OnceLock::new();
        let text = self.plain.as_deref().or(self.synced.as_deref()).unwrap_or("");
        self.is_marked_romaji()
            && text.chars().any(|c| c.is_ascii_alphabetic())
            && !re(r"(?i)\b(?:translation|translated|terjemahan)\b", &TRANSLATION).is_match(&format!("{} {}", self.track, self.album))
            && !has_japanese(text)
    }

    /// Bahasa Jepang: kode bahasa ja, atau (tanpa kode) ada kana. Kanji saja bisa Mandarin.
    fn is_japanese(&self) -> bool {
        let language = self.language.trim().replace('_', "-").to_ascii_lowercase();
        if language == "ja" || language.starts_with("ja-") {
            return true;
        }
        if !language.is_empty() && language != "und" && language != "unknown" {
            return false;
        }
        match &self.lines {
            Some(lines) => lines.iter().any(|l| has_kana(&l.text)),
            None => has_kana(self.synced.as_deref().or(self.plain.as_deref()).unwrap_or("")),
        }
    }
}

struct Candidate {
    record: Record,
    score: f64,
    confident: bool,
}

/// Cari kandidat lirik (paling banyak 12). Yang `automatic` hanya kandidat yakin (judul, artis,
/// dan durasi cocok); kandidat lemah sering berupa cover atau terjemahan lagu lain.
/// `identify_song`: kenali lagu asli dari judul video dulu (pencarian otomatis); pencarian manual
/// dari jendela lirik memakai judul/artis yang diketik apa adanya.
/// `mode`: lagu Jepang (Otomatis/Romaji) juga mencari romaji berwaktu Better Lyrics; Unison menambah
/// kandidat bila LRCLIB belum memberi hasil berwaktu yang yakin.
pub fn search_choices(title: &str, artist: &str, album: &str, duration: f64, identify_song: bool, mode: Mode) -> Result<Vec<Choice>, String> {
    let (raw_title, raw_artist) = (title, artist);
    let (mut title, mut artist) = clean_query(title, artist);
    let mut duration = duration;
    // Video YouTube tidak punya album dan judulnya sering berbeda dari judul rilis:
    // kenali lagu aslinya dulu agar LRCLIB menerima judul, artis, dan durasi yang benar.
    if identify_song && album.is_empty() {
        if let Some(song) = identify(raw_title, raw_artist, duration) {
            title = song.title;
            artist = song.artist;
            if song.duration > 0.0 {
                duration = song.duration;
            }
        }
    }
    let mut note: Option<String> = None;
    let mut records = lrclib_find(&title, &artist, album, duration).unwrap_or_else(|e| {
        note = Some(e);
        Vec::new()
    });
    let mut asked = Vec::new();
    // Mode Lirik asli: rekaman yang sudah romaji tidak dipakai.
    let eligible = |records: &[Record]| -> Vec<Record> { records.iter().filter(|r| mode != Mode::Original || !r.is_marked_romaji()).cloned().collect() };
    let ready = |records: &[Record]| {
        let ranked = rank(eligible(records), &title, &artist, duration);
        auto_choice(&ranked).is_some_and(|c| c.record.instrumental || c.record.has_timing())
    };
    if !ready(&records) {
        add_unison(&mut records, &mut asked, (&title, &artist, album, duration), mode, &mut note);
    }
    let original = auto_choice(&rank(records.clone(), &title, &artist, duration)).map(|c| c.record.clone());
    let want_romaji = match mode {
        Mode::Romaji => true,
        Mode::Original => false,
        Mode::Auto => original.as_ref().map_or(has_kana(&title), Record::is_japanese),
    };
    let mut better = None;
    if want_romaji && !original.as_ref().is_some_and(|r| r.instrumental || r.is_marked_romaji()) {
        // Pakai judul/artis/album/durasi rekaman yang cocok bila ada (lebih tepat dari judul video).
        let canonical = match &original {
            Some(r) => (
                r.track.clone(),
                r.artist.clone(),
                if r.album.trim().is_empty() { album.to_string() } else { r.album.clone() },
                if r.duration > 0.0 { r.duration } else { duration },
            ),
            None => (title.clone(), artist.clone(), album.to_string(), duration),
        };
        match sources::better_lyrics(&canonical.0, &canonical.1, &canonical.2, canonical.3) {
            // Tanpa skor yakin tidak dipakai otomatis, walau LRCLIB kosong: bisa lagu lain.
            Ok((record, confident)) => better = Some((record, confident)),
            Err(e) => note = note.or(Some(e)),
        }
        if !better.as_ref().is_some_and(|b| b.1) {
            add_unison(&mut records, &mut asked, (&canonical.0, &canonical.1, &canonical.2, canonical.3), mode, &mut note);
            // Langkah terakhir: rekaman LRCLIB yang ditandai romaji (judul/album "romaji", "romanized").
            if !ready(&records) && !records.iter().any(Record::is_romaji) {
                records.extend(lrclib_romaji(&title, &artist));
            }
        }
    }
    let better_automatic = better.as_ref().is_some_and(|b| b.1);
    let better_choice = better.and_then(|(record, automatic)| {
        Some(Choice {
            label: format!("Better Lyrics · romaji berwaktu · {} — {}{}", record.track, record.artist, if automatic { "" } else { " · periksa hasil" }),
            lyrics: to_lyrics(&record)?,
            automatic,
        })
    });
    let ranked = rank(eligible(&records), &title, &artist, duration);
    let automatic = auto_choice(&ranked).filter(|_| !better_automatic).map(|c| c as *const Candidate);
    let choices: Vec<Choice> = better_choice
        .into_iter()
        .chain(ranked.iter().take(12).filter_map(|c| {
            let lyrics = to_lyrics(&c.record)?;
            let kind = kind(&lyrics);
            let minutes = if c.record.duration > 0.0 {
                format!(" · {}:{:02}", c.record.duration as u64 / 60, c.record.duration as u64 % 60)
            } else {
                String::new()
            };
            let romaji = if c.record.is_marked_romaji() { " · romaji" } else { "" };
            Some(Choice {
                label: format!("{} · {:.0}% mirip · {} — {}{} · {}{romaji}", c.record.source, c.score * 100.0, c.record.track, c.record.artist, minutes, kind),
                lyrics,
                automatic: automatic == Some(c as *const Candidate),
            })
        }))
        .collect();
    match note {
        // Tidak ada hasil sama sekali dan ada layanan yang gagal: tampilkan alasannya.
        Some(note) if choices.is_empty() => Err(note),
        _ => Ok(choices),
    }
}

/// Tambah kandidat Unison (maks. dua pencarian berbeda per lagu).
fn add_unison(records: &mut Vec<Record>, asked: &mut Vec<String>, query: (&str, &str, &str, f64), mode: Mode, note: &mut Option<String>) {
    let (title, artist, album, duration) = query;
    let key = format!("{title}\u{1f}{artist}\u{1f}{album}\u{1f}{}", duration.round());
    if asked.len() >= 2 || asked.contains(&key) {
        return;
    }
    asked.push(key);
    match sources::unison_find(title, artist, album, duration, mode != Mode::Original) {
        Ok(mut found) => {
            found.append(records);
            found.truncate(20);
            *records = found;
        }
        Err(e) => {
            note.get_or_insert(e);
        }
    }
}

fn kind(lyrics: &Lyrics) -> &'static str {
    if lyrics.is_synced() {
        "berwaktu"
    } else if lyrics.instrumental {
        "instrumental"
    } else {
        "teks biasa"
    }
}

/// Lirik yang pernah tersimpan, paling mirip dengan judul/artis lebih dulu (tombol "Lirik tersimpan…").
pub fn saved_choices(title: &str, artist: &str) -> Vec<Choice> {
    let Some(dir) = cache_path("").and_then(|p| p.parent().map(|d| d.to_path_buf())) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(f64, std::time::SystemTime, String, String, Lyrics)> = Vec::new();
    for entry in entries.flatten().take(5000) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let identity = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.get("identity")?.as_str().map(str::to_string));
        let Some(identity) = identity else { continue };
        let Some(lyrics) = cache_read(&identity) else { continue };
        let mut parts = identity.split('\u{1f}');
        let (song, singer) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
        let score = if artist.trim().is_empty() {
            similarity(title, &song)
        } else {
            similarity(title, &song) * 0.75 + similarity(artist, &singer) * 0.25
        };
        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        found.push((score, modified, song, singer, lyrics));
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
    found
        .into_iter()
        .take(12)
        .map(|(score, _, song, singer, lyrics)| Choice {
            label: format!("Lirik tersimpan · {:.0}% mirip · {song} — {singer} · {}", score * 100.0, kind(&lyrics)),
            lyrics,
            automatic: false,
        })
        .collect()
}

/// Terapkan jenis lirik: romaji untuk lagu Jepang (Otomatis/Romaji), atau teks asli.
pub fn apply_mode(lyrics: &mut Lyrics, mode: Mode) {
    if mode != Mode::Original {
        romanize(lyrics);
    }
}

pub fn identity_of(title: &str, artist: &str, album: &str) -> String {
    identity(title, artist, album)
}

/// Simpan lirik pilihan (termasuk koreksi waktu) untuk lagu ini.
pub fn save(identity: &str, lyrics: &Lyrics) {
    cache_write(identity, lyrics);
}

fn lrclib(path: &str, retries: u32) -> Result<Option<Value>, String> {
    // Satu permintaan per 300 ms ke LRCLIB, sesuai jeda minimal versi WPF.
    static NEXT: OnceLock<Mutex<Instant>> = OnceLock::new();
    let next = NEXT.get_or_init(|| Mutex::new(Instant::now()));
    {
        let mut next = next.lock().unwrap();
        let now = Instant::now();
        if *next > now {
            thread::sleep(*next - now);
        }
        *next = Instant::now() + Duration::from_millis(300);
    }
    for attempt in 0..=retries {
        let response = http::get(&format!("https://lrclib.net/{path}"))
            .map_err(|_| "Layanan lirik belum dapat dihubungi".to_string())?;
        match response.status {
            200 => return Ok(serde_json::from_slice(&response.body).ok()),
            404 => return Ok(None),
            429 => return Err("Pencarian lirik sementara dibatasi layanan. Coba lagi nanti.".into()),
            503 if attempt < retries => thread::sleep(Duration::from_secs(1 << attempt)),
            _ => return Err("Layanan lirik sedang sibuk".into()),
        }
    }
    Err("Layanan lirik sedang sibuk".into())
}

fn lrclib_find(title: &str, artist: &str, album: &str, duration: f64) -> Result<Vec<Record>, String> {
    if !artist.trim().is_empty() {
        let mut path = format!("api/get?track_name={}&artist_name={}", http::encode(title), http::encode(artist));
        if !album.is_empty() {
            path += &format!("&album_name={}", http::encode(album));
        }
        if (1.0..=3600.0).contains(&duration) {
            path += &format!("&duration={}", duration.round());
        }
        // api/get membalas 503 untuk lagu yang tidak dikenalnya: lanjut ke pencarian biasa, tanpa coba ulang.
        if let Ok(Some(value)) = lrclib(&path, 0) {
            if let Some(exact) = Record::from_json(&value) {
                return Ok(vec![exact]);
            }
        }
    }
    let mut path = format!("api/search?track_name={}", http::encode(title));
    if !artist.trim().is_empty() {
        path += &format!("&artist_name={}", http::encode(artist));
    }
    let mut records = parse_records(lrclib(&path, 2)?);
    if records.is_empty() {
        records = parse_records(lrclib(&format!("api/search?q={}", http::encode(title)), 2)?);
    }
    Ok(records)
}

/// Rekaman romaji di LRCLIB: cari judul/artis, lalu "judul artis romaji". Gagal = tidak ada tambahan.
fn lrclib_romaji(title: &str, artist: &str) -> Vec<Record> {
    let mut path = format!("api/search?track_name={}", http::encode(title));
    if !artist.trim().is_empty() {
        path += &format!("&artist_name={}", http::encode(artist));
    }
    let mut found: Vec<Record> = parse_records(lrclib(&path, 0).ok().flatten()).into_iter().filter(Record::is_romaji).collect();
    if found.is_empty() {
        let query = http::encode(&format!("{title} {artist} romaji"));
        found = parse_records(lrclib(&format!("api/search?q={query}"), 0).ok().flatten()).into_iter().filter(Record::is_romaji).collect();
    }
    found
}

fn parse_records(value: Option<Value>) -> Vec<Record> {
    let Some(Value::Array(items)) = value else { return Vec::new() };
    items.iter().take(20).filter_map(Record::from_json).collect()
}

fn rank(records: Vec<Record>, title: &str, artist: &str, duration: f64) -> Vec<Candidate> {
    let mut ranked: Vec<Candidate> = records
        .into_iter()
        .filter(|r| !is_translation(&format!("{} {}", r.track, r.album)))
        .map(|record| {
            let title_score = title_score(&record.track, &record.artist, title);
            let artist_score = similarity(&clean_artist(&record.artist), artist);
            let mut score = if artist.trim().is_empty() { title_score * 0.85 } else { 0.75 * title_score + 0.25 * artist_score };
            let fits = duration <= 0.0 || (record.duration > 0.0 && (record.duration - duration).abs() <= 3.0);
            if !fits {
                score *= 0.8;
            }
            let confident = title_score >= 0.90 && artist_score >= 0.82 && score >= 0.90 && fits;
            Candidate { record, score, confident }
        })
        // Rekaman romaji sering berjudul lain ("... (Romanized)"): tetap dipakai bila artisnya cocok.
        .filter(|c| c.score >= 0.40 || (c.record.is_marked_romaji() && similarity(&c.record.artist, artist) >= 0.85))
        .collect();
    ranked.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(b.record.has_timing().cmp(&a.record.has_timing()))
    });
    ranked
}

/// Pilihan otomatis hanya bila kandidat teratas yakin dan jelas lebih unggul dari lagu lain.
fn auto_choice(choices: &[Candidate]) -> Option<&Candidate> {
    let first = choices.first().filter(|c| c.confident)?;
    let next = choices.iter().skip(1).find(|x| {
        normalize(&x.record.track) != normalize(&first.record.track)
            || normalize(&x.record.artist) != normalize(&first.record.artist)
            || (x.record.duration - first.record.duration).abs() > 3.0
    });
    match next {
        Some(next) if first.score - next.score < 0.06 => None,
        _ => Some(first),
    }
}

fn to_lyrics(record: &Record) -> Option<Lyrics> {
    let source = record.source.to_string();
    if let Some(lines) = record.lines.clone().filter(|l| !l.is_empty()) {
        return Some(Lyrics { lines, source, ..Default::default() });
    }
    if let Some(lines) = record.synced.as_deref().and_then(parse_lrc) {
        return Some(Lyrics { lines, instrumental: record.instrumental, source, ..Default::default() });
    }
    if record.instrumental {
        return Some(Lyrics { instrumental: true, source, ..Default::default() });
    }
    let plain: Vec<String> = record
        .plain
        .as_deref()
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(MAX_LINES)
        .map(str::to_string)
        .collect();
    (!plain.is_empty()).then_some(Lyrics { plain, source, ..Default::default() })
}

/// Parser LRC: [mm:ss.xx]teks, beberapa stempel per baris, tag kata <mm:ss.xx>, dan [offset:ms].
pub fn parse_lrc(text: &str) -> Option<Vec<Line>> {
    static STAMP: OnceLock<Regex> = OnceLock::new();
    static WORD: OnceLock<Regex> = OnceLock::new();
    static OFFSET: OnceLock<Regex> = OnceLock::new();
    let stamp = STAMP.get_or_init(|| Regex::new(r"\[(\d{1,4}):(\d{2})(?:[.:](\d{1,3}))?\]").unwrap());
    let word = WORD.get_or_init(|| Regex::new(r"<\d{1,3}:\d{2}(?:[.:]\d{1,3})?>").unwrap());
    let offset_re = OFFSET.get_or_init(|| Regex::new(r"(?i)\[offset:([+-]?\d{1,8})\]").unwrap());
    let offset = offset_re
        .captures(text)
        .and_then(|c| c[1].parse::<f64>().ok())
        .map(|ms| ms / 1000.0)
        .unwrap_or(0.0);
    let mut lines: Vec<Line> = Vec::new();
    for raw in text.lines().take(MAX_LINES + 32) {
        let stamps: Vec<_> = stamp.captures_iter(raw).collect();
        if stamps.is_empty() || stamps.len() > 8 {
            continue;
        }
        let words = word.replace_all(&stamp.replace_all(raw, ""), "").trim().to_string();
        for c in stamps {
            let Ok(seconds) = c[2].parse::<u32>() else { continue };
            if seconds >= 60 {
                continue;
            }
            let minutes: f64 = c[1].parse().unwrap_or(0.0);
            let fraction = c.get(3).map_or(0.0, |f| f.as_str().parse::<f64>().unwrap_or(0.0) / 10f64.powi(f.as_str().len() as i32));
            let time = minutes * 60.0 + seconds as f64 + fraction - offset;
            lines.push(Line { seconds: time.max(0.0), text: words.clone() });
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.sort_by(|a, b| a.seconds.partial_cmp(&b.seconds).unwrap_or(std::cmp::Ordering::Equal));
    // Baris dengan waktu sama digabung: "baris A / baris B".
    let mut merged: Vec<Line> = Vec::with_capacity(lines.len());
    for line in lines {
        match merged.last_mut() {
            Some(last) if (last.seconds - line.seconds).abs() < 1e-6 => {
                if last.text != line.text {
                    last.text = format!("{} / {}", last.text, line.text);
                }
            }
            _ => merged.push(line),
        }
    }
    merged.truncate(MAX_LINES);
    Some(merged)
}

// ---------------------------------------------------------------------------------------------
// Pembersihan dan pencocokan judul (SongMatcher)

fn re(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn limit(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn clean_title(text: &str) -> String {
    static VIDEO: OnceLock<Regex> = OnceLock::new();
    static ROMAN: OnceLock<Regex> = OnceLock::new();
    static MV: OnceLock<Regex> = OnceLock::new();
    static EMPTY: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    let text = limit(text, 512).replace('\u{200B}', " ");
    let text = re(
        r"(?i)\b(?:official\s+(?:music\s+)?(?:video|audio|lyric\s+video|mv)|music\s+video|lyric\s+video|lyrics\s+video|visuali[sz]er|official\s+mv|hd|4k)\b",
        &VIDEO,
    )
    .replace_all(&text, "");
    let text = re(r"(?i)\b(?:romaji|romanized|romanised|romanization|romanisation)\b", &ROMAN).replace_all(&text, "");
    let text = re(r"(?i)(?:\s+|[\(\[【])(?:M/?V|PV)(?:[\)\]】])?\s*$", &MV).replace_all(&text, "");
    let text = re(r"[\(\[【]\s*[\)\]】]", &EMPTY).replace_all(&text, "");
    re(r"\s+", &SPACES).replace_all(&text, " ").trim_matches(|c| c == ' ' || c == '-' || c == '–' || c == '|').to_string()
}

pub fn clean_artist(text: &str) -> String {
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    re(r"(?i)\s*(?:-\s*Topic|Official(?:\s+Channel)?)\s*$", &SUFFIX).replace_all(&limit(text, 256), "").trim().to_string()
}

pub fn clean_query(title: &str, artist: &str) -> (String, String) {
    static FRAMED: OnceLock<Regex> = OnceLock::new();
    static AGE: OnceLock<Regex> = OnceLock::new();
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    let mut title = clean_title(title);
    let mut artist = clean_artist(artist);
    // Format YouTube seperti Artis『Judul』Official Music Video.
    if let Some(c) = re(r"^(?P<artist>[^『「]{1,128})[『「](?P<title>[^』」]{1,512})[』」]\s*$", &FRAMED).captures(&title) {
        let lead = clean_artist(&c["artist"]);
        let age_suffix = artist.to_lowercase().starts_with(&lead.to_lowercase())
            && artist.len() >= lead.len()
            && re(r"^\s*\(\d{1,2}\)\s*$", &AGE).is_match(artist.get(lead.len()..).unwrap_or(""));
        if similarity(&lead, &artist) >= 0.95 || age_suffix || artist.is_empty() {
            let new_title = c["title"].trim().to_string();
            artist = lead;
            title = new_title;
        }
    }
    if let Some(c) = re(r"^(?P<left>.+?)\s+[-–—]\s+(?P<right>.+)$", &SPLIT).captures(&title.clone()) {
        let (left, right) = (c["left"].trim().to_string(), c["right"].trim().to_string());
        if similarity(&clean_artist(&left), &artist) >= 0.95 {
            title = right;
        } else if similarity(&clean_artist(&right), &artist) >= 0.95 {
            title = left; // Contoh: Heart Forecast - Eve MV, artis Eve.
        } else if artist.trim().is_empty() {
            artist = clean_artist(&left);
            title = right;
        }
    }
    (title, artist)
}

fn is_translation(text: &str) -> bool {
    static LANG: OnceLock<Regex> = OnceLock::new();
    static GENIUS: OnceLock<Regex> = OnceLock::new();
    if re(r"(?i)\b(?:english|indonesian|spanish|french|german|portuguese|russian|korean|japanese|chinese)\s+translation\b|\bterjemahan\b", &LANG).is_match(text) {
        return true;
    }
    re(r"(?i)genius\s+(\w*)\s*translations", &GENIUS)
        .captures(text)
        .is_some_and(|c| !c[1].eq_ignore_ascii_case("romanizations"))
}

/// NFKC lalu NFD tanpa tanda diakritik; huruf/angka kecil, selain itu spasi.
fn normalize(text: &str) -> String {
    let decomposed: String = limit(text, 192).nfkc().collect::<String>().nfd().filter(|c| !is_combining_mark(*c)).collect();
    let mapped: String = decomposed.chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' }).collect();
    limit(&mapped.split_whitespace().collect::<Vec<_>>().join(" "), 192)
}

pub fn similarity(first: &str, second: &str) -> f64 {
    let (a, b) = (normalize(first), normalize(second));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let (a, b): (Vec<char>, Vec<char>) = if a.chars().count() >= b.chars().count() {
        (a.chars().collect(), b.chars().collect())
    } else {
        (b.chars().collect(), a.chars().collect())
    };
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let old = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(diagonal + usize::from(a[i - 1] != b[j - 1]));
            diagonal = old;
        }
    }
    let edit = 1.0 - row[b.len()] as f64 / a.len() as f64;
    let (sa, sb): (String, String) = (a.iter().collect(), b.iter().collect());
    let left: HashSet<&str> = sa.split(' ').collect();
    let right: HashSet<&str> = sb.split(' ').collect();
    let dice = 2.0 * left.intersection(&right).count() as f64 / (left.len() + right.len()) as f64;
    edit.max(dice * 0.96)
}

fn aliases(title: &str, artist: &str) -> Vec<String> {
    static PAREN: OnceLock<Regex> = OnceLock::new();
    static CJK: OnceLock<Regex> = OnceLock::new();
    static VERSION_ALIAS: OnceLock<Regex> = OnceLock::new();
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let title = clean_query(title, artist).0;
    let mut out = vec![title.clone()];
    let paren = re(r"[\(（]([^\)）]+)[\)）]", &PAREN);
    let japanese = re(r"[぀-ヿ㐀-鿿]", &CJK).is_match(&title);
    for c in paren.captures_iter(&title).take(4) {
        let alias = c[1].trim().to_string();
        if japanese && !re(r"(?i)\b(?:live|remix|cover|version|ver|acoustic|instrumental|feat|ft)\b", &VERSION_ALIAS).is_match(&alias) {
            out.push(alias);
        }
    }
    // Label versi bermakna dan harus tetap ikut dicocokkan.
    if !re(r"(?i)\b(?:live|remix|cover|acoustic|instrumental|version|ver)\b", &VERSION).is_match(&title) {
        out.push(paren.replace_all(&title, "").trim().to_string());
    }
    let mut seen = HashSet::new();
    out.into_iter().filter(|x| !x.trim().is_empty() && seen.insert(x.clone())).take(6).collect()
}

fn title_score(title: &str, artist: &str, query: &str) -> f64 {
    aliases(title, artist).iter().map(|a| similarity(a, query)).fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------------------------
// Pengenalan lagu asli dari judul video (SongIdentifier, katalog publik Deezer)

struct Identified {
    title: String,
    artist: String,
    duration: f64,
}

fn key(text: &str) -> String {
    text.nfkc().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

const LATIN_NOISE: &str = r"(?i)\b(?:official|music\s*video|video|audio|lyrics?|lirik|mv|pv|4k|hd|\d{3,4}p|sub(?:title)?s?|terjemahan|romaji|eng|english|indo|full\s*ver(?:sion)?|ver\.?|version|cover|live|remaster(?:ed)?|visuali[sz]er|op|ed|opening|ending|ost|anime|nightcore|slowed|reverb|sped\s*up|8d|karaoke|tiktok|color\s*coded)\b";
const CJK_NOISE: &str = "歌詞|字幕|歌ってみた|弾いてみた|叩いてみた|公式|主題歌|オープニング|エンディング|カラオケ|フル";
const SEPARATOR: &str = r"\s[-–—~]\s|[|｜／]|\s/\s";

fn identifier_artist(artist: &str) -> String {
    static VEVO: OnceLock<Regex> = OnceLock::new();
    re(r"(?i)\s*VEVO$", &VEVO).replace_all(&clean_artist(artist), "").trim().to_string()
}

/// Judul bersih (pemisah tetap ada), bagian dalam 『』/「」, dan potongan per pemisah.
fn identifier_clean(raw: &str) -> (String, String, Vec<String>) {
    static QUOTED: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BRACKET: OnceLock<Regex> = OnceLock::new();
    static LATIN: OnceLock<Regex> = OnceLock::new();
    static CJK: OnceLock<Regex> = OnceLock::new();
    static FEAT: OnceLock<Regex> = OnceLock::new();
    static MARKS: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    static SEP: OnceLock<Regex> = OnceLock::new();
    let text = limit(raw, 512);
    let quoted = re(r"[『「]([^』」]{1,128})[』」]", &QUOTED).captures(&text).map(|c| c[1].trim().to_string()).unwrap_or_default();
    let text = re(r"#\S+", &TAG).replace_all(&text, " ");
    let latin = re(LATIN_NOISE, &LATIN);
    let cjk = re(CJK_NOISE, &CJK);
    let text = re(r"[\(\[【（［〔]([^\)\]】）］〕]{0,128})[\)\]】）］〕]", &BRACKET).replace_all(&text, |c: &regex::Captures| {
        if latin.is_match(&c[1]) || cjk.is_match(&c[1]) { " ".to_string() } else { c[0].to_string() }
    });
    let text = re(r"(?i)\s(?:feat\.?|ft\.?|prod\.?\s*by|prod\.)\s[^\(\[【|｜／-]+", &FEAT).replace_all(&text, " ");
    let text = re(r#"[『』「」"]"#, &MARKS).replace_all(&clean_title(&text), " ").to_string();
    let text = re(r"\s+", &SPACES).replace_all(&text, " ").trim_matches(|c| " -–|/".contains(c)).to_string();
    let parts = re(SEPARATOR, &SEP).split(&text).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect();
    (text, quoted, parts)
}

fn identifier_queries(raw_title: &str, raw_artist: &str) -> Vec<String> {
    static SEP: OnceLock<Regex> = OnceLock::new();
    let (whole, quoted, parts) = identifier_clean(raw_title);
    let artist = identifier_artist(raw_artist);
    let flat = re(SEPARATOR, &SEP).replace_all(&whole, " ").trim().to_string();
    let with_artist = if !artist.is_empty() && !key(&flat).contains(&key(&artist)) { format!("{flat} {artist}") } else { flat.clone() };
    let longest = parts.iter().filter(|x| key(x) != key(&artist)).max_by_key(|x| x.len()).cloned().unwrap_or_default();
    let first = if quoted.is_empty() { with_artist } else { format!("{quoted} {artist}") };
    let mut seen = HashSet::new();
    [first, flat, longest]
        .into_iter()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty() && seen.insert(x.to_lowercase()))
        .take(3)
        .collect()
}

fn identify(raw_title: &str, raw_artist: &str, duration: f64) -> Option<Identified> {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let version = re(r"(?i)(?:live|remix|acoustic|instrumental|karaoke|sped\s*up|slowed|remaster(?:ed)?|tour|unplugged)", &VERSION);
    let (whole, quoted, parts) = identifier_clean(raw_title);
    let artist = identifier_artist(raw_artist);
    let context = key(&format!("{raw_title} {artist}"));
    let titles: Vec<String> = parts.into_iter().chain([whole, quoted]).filter(|x| !x.is_empty()).collect();
    let raw_lower = raw_title.to_lowercase();
    for query in identifier_queries(raw_title, raw_artist) {
        let response = http::get(&format!("https://api.deezer.com/search?limit=10&q={}", http::encode(&query))).ok()?;
        if response.status != 200 {
            return None;
        }
        let json: Value = serde_json::from_slice(&response.body).ok()?;
        let Some(items) = json.get("data").and_then(Value::as_array) else { continue };
        let mut best: Option<(f64, Identified)> = None;
        for item in items.iter().take(10) {
            let text = |v: Option<&Value>| v.and_then(Value::as_str).map(|s| limit(s, 256)).unwrap_or_default();
            let mut title = text(item.get("title_short"));
            if title.is_empty() {
                title = text(item.get("title"));
            }
            let song_artist = text(item.get("artist").and_then(|a| a.get("name")));
            let album = text(item.get("album").and_then(|a| a.get("title")));
            let seconds = item.get("duration").and_then(Value::as_f64).filter(|d| (1.0..=3600.0).contains(d)).unwrap_or(0.0);
            if title.is_empty() || song_artist.is_empty() {
                continue;
            }
            let song_key = key(&title);
            let mut score = titles.iter().map(|x| similarity(&title, x)).fold(0.0, f64::max);
            if song_key.chars().count() >= 2 && context.contains(&song_key) {
                score = score.max(0.92);
            }
            let artist_key = key(&song_artist);
            let artist_hit = similarity(&song_artist, &artist) >= 0.8 || (artist_key.chars().count() >= 2 && context.contains(&artist_key));
            // Tanpa artis yang cocok, judul pendek seperti "Lemon" terlalu mudah tertukar.
            if score < 0.8 || !artist_hit {
                continue;
            }
            // MV boleh lebih panjang (intro/outro), tetapi rekaman yang lebih panjang biasanya versi lain.
            let longer = duration > 0.0 && seconds > 0.0 && (seconds - duration > 10.0 || duration - seconds > 90.0);
            let other_version = version.find_iter(&format!("{title} {album}")).any(|m| !raw_lower.contains(&m.as_str().to_lowercase()));
            let score = score - if longer { 0.2 } else { 0.0 } - if other_version { 0.3 } else { 0.0 };
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, Identified { title, artist: song_artist, duration: seconds }));
            }
        }
        if let Some((_, song)) = best {
            return Some(song);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Romaji

fn has_kana(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30FF}'))
}

pub fn has_japanese(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{9FFF}' | '\u{FF66}'..='\u{FF9D}'))
}

/// Lagu Jepang ditampilkan dalam romaji. Baris yang gagal dibaca tetap memakai teks aslinya.
fn romanize(lyrics: &mut Lyrics) {
    if !lyrics.lines.iter().any(|l| has_japanese(&l.text)) {
        return;
    }
    let mut converted = false;
    for line in &mut lyrics.lines {
        if has_japanese(&line.text) {
            let romaji = romaji_line(&line.text);
            if !romaji.is_empty() && !has_japanese(&romaji) {
                line.text = romaji;
                converted = true;
            }
        }
    }
    if converted {
        lyrics.source.push_str(" · romaji lokal");
    }
}

fn is_japanese_run_char(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF66}'..='\u{FF9D}' | '々' | '〆')
}

/// Satu baris: bagian Jepang dibaca per kata, bagian lain (Inggris, angka, tanda baca) dipertahankan.
pub fn romaji_line(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut other = String::new();
    while i < chars.len() {
        if !is_japanese_run_char(chars[i]) {
            other.push(chars[i]);
            i += 1;
            continue;
        }
        add_other(&mut words, &other);
        other.clear();
        let start = i;
        while i < chars.len() && is_japanese_run_char(chars[i]) {
            i += 1;
        }
        let run: Vec<char> = chars[start..i].to_vec();
        // Batas input penganalisis Windows: 100 karakter per panggilan.
        for chunk in run.chunks(100) {
            let chunk: String = chunk.iter().collect();
            let Ok(phonemes) = JapanesePhoneticAnalyzer::GetWords(&HSTRING::from(chunk.as_str())) else { continue };
            for phoneme in phonemes {
                let display = phoneme.DisplayText().map(|s| s.to_string()).unwrap_or_default();
                let yomi = phoneme.YomiText().map(|s| s.to_string()).unwrap_or_default();
                let yomi = if yomi.is_empty() { display.clone() } else { yomi };
                let phrase_start = phoneme.IsPhraseStart().unwrap_or(true);
                // Partikel ditulis menurut bunyinya: は = wa, へ = e, を = wo.
                let romaji = if !phrase_start && display == "は" {
                    "wa".to_string()
                } else if !phrase_start && display == "へ" {
                    "e".to_string()
                } else if display == "を" {
                    "wo".to_string()
                } else {
                    kana_to_romaji(&yomi)
                };
                if romaji.is_empty() {
                    continue;
                }
                // Konsonan ganda (っ) di akhir kata digabung dengan kata berikutnya.
                match words.last_mut() {
                    Some(last) if last.ends_with('\u{1}') => {
                        last.pop();
                        let first = romaji.chars().next().unwrap_or('a');
                        if romaji.starts_with("ch") {
                            last.push('t');
                        } else if !"aiueo".contains(first) {
                            last.push(first);
                        }
                        last.push_str(&romaji);
                    }
                    _ => words.push(romaji),
                }
            }
        }
    }
    add_other(&mut words, &other);
    static BEFORE: OnceLock<Regex> = OnceLock::new();
    static AFTER: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    let line = words.join(" ").replace('\u{1}', "");
    let line = re(r"\s+([,.!?~)\]」』])", &BEFORE).replace_all(&line, "$1");
    let line = re(r"([(\[「『])\s+", &AFTER).replace_all(&line, "$1");
    re(r"\s{2,}", &SPACES).replace_all(&line, " ").trim().to_string()
}

fn add_other(words: &mut Vec<String>, text: &str) {
    let text = text
        .replace('\u{3000}', " ")
        .replace('、', ", ")
        .replace('。', ". ")
        .replace('！', "! ")
        .replace('？', "? ")
        .replace(['～', '〜'], "~")
        .replace('・', " ")
        .replace('（', "(")
        .replace('）', ")")
        .replace(['「', '」', '『', '』'], "\"")
        .replace('…', "...");
    words.extend(text.split(' ').filter(|p| !p.is_empty()).map(str::to_string));
}

fn kana_table() -> &'static std::collections::HashMap<String, &'static str> {
    static TABLE: OnceLock<std::collections::HashMap<String, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = std::collections::HashMap::new();
        let mut row = |kana: &str, romaji: &[&'static str]| {
            for (k, r) in kana.chars().zip(romaji) {
                t.insert(k.to_string(), *r);
            }
        };
        row("あいうえお", &["a", "i", "u", "e", "o"]);
        row("かきくけこ", &["ka", "ki", "ku", "ke", "ko"]);
        row("がぎぐげご", &["ga", "gi", "gu", "ge", "go"]);
        row("さしすせそ", &["sa", "shi", "su", "se", "so"]);
        row("ざじずぜぞ", &["za", "ji", "zu", "ze", "zo"]);
        row("たちつてと", &["ta", "chi", "tsu", "te", "to"]);
        row("だぢづでど", &["da", "ji", "zu", "de", "do"]);
        row("なにぬねの", &["na", "ni", "nu", "ne", "no"]);
        row("はひふへほ", &["ha", "hi", "fu", "he", "ho"]);
        row("ばびぶべぼ", &["ba", "bi", "bu", "be", "bo"]);
        row("ぱぴぷぺぽ", &["pa", "pi", "pu", "pe", "po"]);
        row("まみむめも", &["ma", "mi", "mu", "me", "mo"]);
        row("やゆよ", &["ya", "yu", "yo"]);
        row("らりるれろ", &["ra", "ri", "ru", "re", "ro"]);
        row("わゐゑを", &["wa", "wi", "we", "o"]);
        row("ぁぃぅぇぉゃゅょゎ", &["a", "i", "u", "e", "o", "ya", "yu", "yo", "wa"]);
        row("ゔゕゖ", &["vu", "ka", "ke"]);
        // Gabungan yōon: き+ゃ = kya, し+ゃ = sha, dst.
        for (kana, head) in [("き", "ky"), ("ぎ", "gy"), ("し", "sh"), ("じ", "j"), ("ち", "ch"), ("ぢ", "j"), ("に", "ny"), ("ひ", "hy"), ("び", "by"), ("ぴ", "py"), ("み", "my"), ("り", "ry")] {
            for (small, vowel) in [("ゃ", "a"), ("ゅ", "u"), ("ょ", "o")] {
                t.insert(format!("{kana}{small}"), Box::leak(format!("{head}{vowel}").into_boxed_str()));
            }
        }
        for (k, r) in [
            ("しぇ", "she"), ("じぇ", "je"), ("ちぇ", "che"), ("てぃ", "ti"), ("でぃ", "di"), ("とぅ", "tu"), ("どぅ", "du"),
            ("ふぁ", "fa"), ("ふぃ", "fi"), ("ふぇ", "fe"), ("ふぉ", "fo"), ("ふゅ", "fyu"), ("うぃ", "wi"), ("うぇ", "we"),
            ("うぉ", "wo"), ("つぁ", "tsa"), ("つぃ", "tsi"), ("つぇ", "tse"), ("つぉ", "tso"), ("ゔぁ", "va"), ("ゔぃ", "vi"),
            ("ゔぇ", "ve"), ("ゔぉ", "vo"), ("いぇ", "ye"), ("くぁ", "kwa"),
        ] {
            t.insert(k.to_string(), r);
        }
        t
    })
}

/// Kana ke romaji Hepburn: っ menggandakan konsonan, ー memanjangkan vokal,
/// dan ん sebelum vokal/y diberi apostrof (kan'i, bukan kani).
pub fn kana_to_romaji(input: &str) -> String {
    let table = kana_table();
    // Katakana -> hiragana.
    let s: Vec<char> = input
        .chars()
        .map(|c| if ('ァ'..='ヶ').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c })
        .collect();
    let mut out = String::with_capacity(s.len() * 2);
    let mut double_next = false;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == 'っ' {
            double_next = true;
            i += 1;
            continue;
        }
        if c == 'ん' {
            out.push('n');
            if let Some(after) = s.get(i + 1).and_then(|n| table.get(&n.to_string())) {
                if after.starts_with(|v: char| "aiueoy".contains(v)) {
                    out.push('\'');
                }
            }
            i += 1;
            continue;
        }
        if c == 'ー' || c == 'ｰ' {
            if let Some(v) = out.chars().last().filter(|v| "aiueo".contains(*v)) {
                out.push(v);
            }
            i += 1;
            continue;
        }
        let pair: Option<&str> = s.get(i + 1).and_then(|n| table.get(&format!("{c}{n}")).copied());
        let romaji = if let Some(pair) = pair {
            i += 1;
            Some(pair)
        } else {
            table.get(&c.to_string()).copied()
        };
        i += 1;
        let Some(romaji) = romaji else {
            // Tanda kana tanpa bunyi dibuang; karakter lain dipertahankan.
            if !c.is_whitespace() && !('\u{3040}'..='\u{30FF}').contains(&c) {
                out.push(c);
            }
            double_next = false;
            continue;
        };
        if double_next {
            if romaji.starts_with("ch") {
                out.push('t');
            } else if let Some(first) = romaji.chars().next().filter(|f| !"aiueo".contains(*f)) {
                out.push(first);
            }
            double_next = false;
        }
        out.push_str(romaji);
    }
    // っ di akhir kata disambung ke kata berikutnya.
    if double_next {
        out.push('\u{1}');
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cache: satu file JSON kecil per lagu di %LocalAppData%\NotchRust\lyrics

fn identity(title: &str, artist: &str, album: &str) -> String {
    format!("{}\u{1f}{}\u{1f}{}", normalize(title), normalize(&clean_artist(artist)), normalize(album))
}

fn cache_path(identity: &str) -> Option<PathBuf> {
    // FNV-1a 64-bit: nama file stabil tanpa memuat judul lagu.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in identity.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("NotchRust").join("lyrics").join(format!("{hash:016x}.json")))
}

fn cache_read(identity: &str) -> Option<Lyrics> {
    let text = std::fs::read_to_string(cache_path(identity)?).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("identity")?.as_str()? != identity {
        return None;
    }
    let lines = value
        .get("lines")?
        .as_array()?
        .iter()
        .filter_map(|l| Some(Line { seconds: l.get(0)?.as_f64()?, text: l.get(1)?.as_str()?.to_string() }))
        .take(MAX_LINES)
        .collect();
    let plain = value
        .get("plain")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter_map(|x| x.as_str().map(str::to_string)).take(MAX_LINES).collect())
        .unwrap_or_default();
    let source = value.get("source").and_then(Value::as_str).unwrap_or("LRCLIB");
    Some(Lyrics {
        lines,
        plain,
        instrumental: value.get("instrumental").and_then(Value::as_bool).unwrap_or(false),
        source: if source.ends_with("tersimpan") { source.to_string() } else { format!("{source} · tersimpan") },
        offset: value.get("offset").and_then(Value::as_f64).filter(|o| o.abs() <= 600.0).unwrap_or(0.0),
    })
}

fn cache_remove(identity: &str) {
    if let Some(path) = cache_path(identity) {
        let _ = std::fs::remove_file(path);
    }
}

fn cache_write(identity: &str, lyrics: &Lyrics) {
    let Some(path) = cache_path(identity) else { return };
    let value = json!({
        "identity": identity,
        "source": lyrics.source,
        "instrumental": lyrics.instrumental,
        "offset": lyrics.offset,
        "lines": lyrics.lines.iter().map(|l| json!([l.seconds, l.text])).collect::<Vec<_>>(),
        "plain": lyrics.plain,
    });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, value.to_string()).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(track: &str, artist: &str, duration: f64) -> Record {
        Record { track: track.into(), artist: artist.into(), duration, synced: Some("[00:01.00]a".into()), ..Default::default() }
    }

    #[test]
    fn auto_choice_only_when_sure_and_clearly_ahead() {
        // Cocok judul, artis, durasi: dipakai otomatis.
        let ranked = rank(vec![record("Shape of You", "Ed Sheeran", 233.0)], "Shape of You", "Ed Sheeran", 233.0);
        assert!(auto_choice(&ranked).is_some());
        // Durasi beda jauh: tidak yakin.
        let ranked = rank(vec![record("Shape of You", "Ed Sheeran", 300.0)], "Shape of You", "Ed Sheeran", 233.0);
        assert!(auto_choice(&ranked).is_none());
        // Artis lain: tidak yakin.
        let ranked = rank(vec![record("Shape of You", "Someone Else", 233.0)], "Shape of You", "Ed Sheeran", 233.0);
        assert!(auto_choice(&ranked).is_none());
        // Dua rekaman berbeda dengan skor hampir sama: ragu, jadi tidak otomatis.
        let ranked = rank(
            vec![record("Shape of You", "Ed Sheeran", 233.0), record("Shape of You", "Ed Sheeran", 240.0)],
            "Shape of You",
            "Ed Sheeran",
            0.0,
        );
        assert!(auto_choice(&ranked).is_none());
    }

    #[test]
    fn lrc_parses_times_and_offset() {
        let lines = parse_lrc("[offset:500]\n[00:01.00]satu\n[01:02.50]dua\nbukan baris").unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["satu", "dua"]);
        assert!((lines[0].seconds - 0.5).abs() < 1e-3);
        assert!((lines[1].seconds - 62.0).abs() < 1e-3);
    }

    #[test]
    fn index_at_finds_current_line() {
        let lyrics = Lyrics { lines: parse_lrc("[00:01.00]a\n[00:05.00]b").unwrap(), ..Default::default() };
        assert_eq!(lyrics.index_at(0.5), None);
        assert_eq!(lyrics.index_at(1.0), Some(0));
        assert_eq!(lyrics.index_at(9.0), Some(1));
    }

    #[test]
    fn clean_query_splits_artist_and_title() {
        assert_eq!(clean_query("Tulus - Hati-Hati di Jalan (Official Lyric Video)", "TULUS").0, "Hati-Hati di Jalan");
        assert_eq!(clean_query("YOASOBI「アイドル」Official Music Video", "YOASOBI"), ("アイドル".to_string(), "YOASOBI".to_string()));
        assert_eq!(clean_artist("Eve - Topic"), "Eve");
    }

    #[test]
    fn similarity_ignores_case_and_accents() {
        assert_eq!(similarity("Café", "cafe"), 1.0);
        assert!(similarity("Shape of You", "Shape of You (Acoustic)") > 0.6);
        assert!(similarity("Lemon", "Peach") < 0.5);
    }

    #[test]
    fn identifier_queries_drop_video_noise() {
        let q = identifier_queries("Ed Sheeran - Shape of You [Official Video]", "Ed SheeranVEVO");
        assert_eq!(q[0], "Ed Sheeran Shape of You");
        let q = identifier_queries("YOASOBI「アイドル」Official Music Video", "Ayase / YOASOBI");
        assert!(q[0].starts_with("アイドル"));
    }

    #[test]
    fn kana_converts_to_hepburn() {
        assert_eq!(kana_to_romaji("きょう"), "kyou");
        assert_eq!(kana_to_romaji("がっこう"), "gakkou");
        assert_eq!(kana_to_romaji("かんい"), "kan'i");
        assert_eq!(kana_to_romaji("コーヒー"), "koohii");
        assert_eq!(kana_to_romaji("まっちゃ"), "matcha");
    }

    #[test]
    fn lrc_export_roundtrips_with_offset() {
        let lyrics = Lyrics { lines: parse_lrc("[00:01.50]satu\n[01:02.00]dua").unwrap(), offset: 0.5, ..Default::default() };
        let text = lyrics.to_file();
        assert_eq!(text, "[00:02.00]satu\r\n[01:02.50]dua");
        let back = Lyrics::from_file(&text).unwrap();
        assert!((back.lines[1].seconds - 62.5).abs() < 1e-6);
        assert_eq!(Lyrics::from_file("baris satu\n\nbaris dua").unwrap().plain, ["baris satu", "baris dua"]);
    }

    #[test]
    fn romaji_records_are_marked_and_not_translations() {
        let record = |track: &str, plain: &str| Record { track: track.into(), plain: Some(plain.into()), ..Default::default() };
        assert!(record("Lemon (Romanized)", "yume naraba").is_romaji());
        assert!(!record("Lemon", "yume naraba").is_romaji());
        assert!(!record("Lemon (Romaji) English Translated", "if it were a dream").is_romaji());
        assert!(!record("Lemon romaji", "夢ならば").is_romaji());
    }

    #[test]
    fn translations_are_rejected() {
        assert!(is_translation("Lemon (English Translation)"));
        assert!(is_translation("Genius English Translations"));
        assert!(!is_translation("Genius Romanizations"));
    }
}
