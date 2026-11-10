// Romaji berwaktu dari Better Lyrics (TTML ja-Latn) serta lirik dan romaji dari Unison: port
// BetterLyricsClient, TtmlRomajiParser, dan UnisonClient NotchMusic. Respons dibatasi ukurannya;
// bila gagal, sumber lain yang dipakai (hasil sebagian tidak pernah ditampilkan).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{has_japanese, rank, Line, Lyrics, Record, MAX_LINES, MAX_SOURCE_CHARS};
use crate::http;

const MAX_XML_CHARS: usize = 384_000;
const MAX_LINE_CHARS: usize = 4096;

// ---------------------------------------------------------------------------------------------
// XML minimal untuk TTML

#[derive(Debug, PartialEq)]
enum Event {
    Start { name: String, attrs: Vec<(String, String)>, empty: bool },
    End(String),
    Text(String),
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find(';')? + i;
        let entity = &rest[i + 1..end];
        out.push(match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = match entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => entity.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(code)?
            }
        });
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Elemen, atribut, teks, dan CDATA. DTD ditolak dan kedalaman dibatasi 32, seperti versi WPF.
fn parse_xml(xml: &str) -> Option<Vec<Event>> {
    let mut events = Vec::new();
    let mut rest = xml;
    let mut depth = 0usize;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            events.push(Event::Text(unescape(rest)?));
            break;
        };
        if open > 0 {
            events.push(Event::Text(unescape(&rest[..open])?));
        }
        rest = &rest[open..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = &body[body.find("-->")? + 3..];
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>")?;
            events.push(Event::Text(body[..end].to_string()));
            rest = &body[end + 3..];
        } else if rest.starts_with("<!") {
            return None;
        } else if let Some(body) = rest.strip_prefix("<?") {
            rest = &body[body.find("?>")? + 2..];
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body.find('>')?;
            events.push(Event::End(local(body[..end].trim()).to_string()));
            depth = depth.checked_sub(1)?;
            rest = &body[end + 1..];
        } else {
            let body = &rest[1..];
            // '>' penutup tag, di luar tanda kutip nilai atribut.
            let mut quote = None;
            let mut close = None;
            for (i, c) in body.char_indices() {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quote = Some(c),
                    None if c == '>' => {
                        close = Some(i);
                        break;
                    }
                    None => {}
                }
            }
            let close = close?;
            let mut tag = &body[..close];
            let empty = tag.ends_with('/');
            if empty {
                tag = &tag[..tag.len() - 1];
            }
            let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let name = local(&tag[..name_end]).to_string();
            let mut attrs = Vec::new();
            let mut list = tag[name_end..].trim_start();
            while !list.is_empty() {
                let eq = list.find('=')?;
                let key = list[..eq].trim().to_string();
                let value = list[eq + 1..].trim_start();
                let q = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                let end = value[1..].find(q)? + 1;
                attrs.push((key, unescape(&value[1..end])?));
                list = value[end + 1..].trim_start();
            }
            if !empty {
                depth += 1;
                if depth > 32 {
                    return None;
                }
            }
            events.push(Event::Start { name, attrs, empty });
            rest = &body[close + 1..];
        }
    }
    Some(events)
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------------------------
// TTML: romaji ja-Latn memakai referensi baris asli

struct Entry {
    key: String,
    begin: Option<f64>,
    end: Option<f64>,
    text: String,
}

/// "12.5s", "1500ms", "01:02.5", "1:02:03.25".
fn parse_time(text: Option<&str>) -> Option<f64> {
    let mut text = text?.trim();
    if text.is_empty() {
        return None;
    }
    let mut factor = 1.0;
    if let Some(ms) = text.strip_suffix("ms") {
        factor = 0.001;
        text = ms;
    } else if let Some(s) = text.strip_suffix('s') {
        text = s;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 || (factor != 1.0 && parts.len() != 1) {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
        let n: f64 = part.parse().ok()?;
        if i > 0 && n >= 60.0 {
            return None;
        }
        total = total * 60.0 + n;
    }
    total *= factor;
    (total <= 86400.0).then_some(total)
}

/// Baca satu <p>/<text> beserta isinya; kembalikan entri dan indeks setelah elemen ini.
fn read_entry(events: &[Event], start: usize) -> Option<(Entry, usize)> {
    let Event::Start { attrs, empty, .. } = &events[start] else { return None };
    let key = attr(attrs, "for")
        .map(str::to_string)
        .or_else(|| attrs.iter().rfind(|(k, _)| matches!(local(k), "key" | "id")).map(|(_, v)| v.clone()))
        .unwrap_or_default();
    let (mut first_span, mut last_span): (Option<f64>, Option<f64>) = (None, None);
    let mut text = String::new();
    let mut next = start + 1;
    if !empty {
        let mut depth = 1;
        while depth > 0 {
            match events.get(next)? {
                Event::Start { name, attrs, empty } => {
                    if name == "span" {
                        if let Some(b) = parse_time(attr(attrs, "begin")) {
                            first_span = Some(first_span.map_or(b, |f| f.min(b)));
                        }
                        if let Some(e) = parse_time(attr(attrs, "end")) {
                            last_span = Some(last_span.map_or(e, |l| l.max(e)));
                        }
                    } else if name == "br" {
                        text.push(' ');
                    }
                    if !empty {
                        depth += 1;
                    }
                }
                Event::End(_) => depth -= 1,
                Event::Text(t) => {
                    if text.len() + t.len() > MAX_LINE_CHARS {
                        return None;
                    }
                    text.push_str(t); // Spasi antarsuku kata dan tanda baca dipertahankan.
                }
            }
            next += 1;
        }
    }
    let begin = parse_time(attr(attrs, "begin")).or(first_span);
    let end = parse_time(attr(attrs, "end")).or(last_span);
    Some((Entry { key, begin, end, text: text.trim().to_string() }, next))
}

/// Baris TTML. `romaji`: hanya berhasil bila tiap baris Jepang punya romaji ja-Latn yang cocok waktunya.
pub(super) fn ttml_lines(xml: &str, duration: f64, romaji: bool) -> Vec<Line> {
    ttml_core(xml, duration, romaji).unwrap_or_default()
}

fn ttml_core(xml: &str, duration: f64, require_romaji: bool) -> Option<Vec<Line>> {
    if xml.is_empty() || xml.len() > MAX_XML_CHARS * 3 || xml.chars().count() > MAX_XML_CHARS {
        return None;
    }
    let events = parse_xml(xml)?;
    let mut romanized: HashMap<String, Entry> = HashMap::new();
    let mut originals: Vec<Entry> = Vec::new();
    let (mut in_body, mut saw_romaji) = (false, false);
    let (mut original_chars, mut romaji_chars) = (0, 0);
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::End(name) if name == "body" => in_body = false,
            Event::Start { name, .. } if name == "body" => in_body = true,
            Event::Start { name, attrs, empty } if require_romaji && name == "transliteration" && !saw_romaji => {
                let language = attr(attrs, "xml:lang").unwrap_or("").to_ascii_lowercase();
                if language == "ja-latn" || language.starts_with("ja-latn-") {
                    saw_romaji = true;
                    let mut depth = if *empty { 0 } else { 1 };
                    let mut j = i + 1;
                    while depth > 0 && j < events.len() {
                        match &events[j] {
                            Event::Start { name, .. } if name == "text" => {
                                let (entry, next) = read_entry(&events, j)?;
                                j = next;
                                if entry.key.is_empty() || entry.text.is_empty() {
                                    continue;
                                }
                                romaji_chars += entry.text.len();
                                if romaji_chars > MAX_SOURCE_CHARS || romanized.len() >= MAX_LINES || romanized.contains_key(&entry.key) {
                                    return None;
                                }
                                romanized.insert(entry.key.clone(), entry);
                                continue;
                            }
                            Event::Start { empty, .. } if !empty => depth += 1,
                            Event::End(_) => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
            }
            Event::Start { name, .. } if in_body && name == "p" => {
                let (entry, next) = read_entry(&events, i)?;
                i = next;
                if !entry.text.is_empty() {
                    original_chars += entry.text.len();
                    if original_chars > MAX_SOURCE_CHARS || originals.len() >= MAX_LINES {
                        return None;
                    }
                    originals.push(entry);
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if originals.is_empty() || (require_romaji && romanized.is_empty()) {
        return None;
    }
    let mut picked: Vec<(f64, Option<f64>, String)> = Vec::with_capacity(originals.len());
    let (mut matched, mut characters) = (0, 0);
    for original in &originals {
        let romaji = if require_romaji { romanized.get(&original.key) } else { None };
        // Lagu Jepang kadang memiliki baris Inggris: boleh dipakai bila tanpa aksara Jepang.
        if require_romaji && romaji.is_none() && has_japanese(&original.text) {
            return None;
        }
        let text = romaji.map_or(&original.text, |r| &r.text);
        if require_romaji && has_japanese(text) {
            return None;
        }
        let begin = original.begin.or(romaji.and_then(|r| r.begin))?;
        let end = original.end.or(romaji.and_then(|r| r.end));
        if end.is_some_and(|e| e < begin) {
            return None;
        }
        if let Some(r) = romaji {
            matched += 1;
            // Jangan menempelkan romaji yang merujuk baris/waktu berbeda.
            let off = r.begin.is_some_and(|rb| rb < begin - 2.0 || end.is_some_and(|e| rb > e + 2.0))
                || r.end.is_some_and(|re| re < begin - 2.0 || end.is_some_and(|e| re > e + 2.0))
                || matches!((r.begin, r.end), (Some(rb), Some(re)) if re < rb);
            if off {
                return None;
            }
        }
        if duration > 0.0 && end.unwrap_or(begin) > duration + 10.0 {
            return None;
        }
        characters += text.len();
        if characters > MAX_SOURCE_CHARS {
            return None;
        }
        picked.push((begin, end, text.clone()));
    }
    if require_romaji && matched == 0 {
        return None;
    }
    picked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut output: Vec<Line> = Vec::with_capacity(picked.len());
    for (i, (start, end, text)) in picked.iter().enumerate() {
        match output.last_mut() {
            Some(last) if (last.seconds - start).abs() < 0.001 => {
                last.text.push_str(" / ");
                last.text.push_str(text);
            }
            _ => output.push(Line { seconds: *start, text: text.clone() }),
        }
        let next = picked.get(i + 1).map_or(f64::INFINITY, |p| p.0);
        // Jeda panjang setelah baris: baris kosong agar lirik lama tidak tertahan.
        if let Some(end) = end.filter(|e| *e > *start && next - *e > 1.5) {
            output.push(Line { seconds: end, text: String::new() });
        }
        if output.len() > MAX_LINES || output.last().is_some_and(|l| l.text.len() > MAX_LINE_CHARS) {
            return None;
        }
    }
    (output.iter().map(|l| l.text.len() + 3).sum::<usize>() <= MAX_SOURCE_CHARS).then_some(output)
}

// ---------------------------------------------------------------------------------------------
// Better Lyrics

/// Jeda yang diminta layanan (429/503) sampai waktu ini.
fn paused(cell: &'static OnceLock<Mutex<Instant>>) -> &'static Mutex<Instant> {
    cell.get_or_init(|| Mutex::new(Instant::now()))
}

/// Romaji berwaktu; `bool` = cukup yakin untuk dipilih otomatis.
pub(super) fn better_lyrics(title: &str, artist: &str, album: &str, duration: f64) -> Result<(Record, bool), String> {
    static PAUSE: OnceLock<Mutex<Instant>> = OnceLock::new();
    if title.trim().is_empty() || artist.trim().is_empty() {
        return Err("Better Lyrics memerlukan judul dan artis.".into());
    }
    if Instant::now() < *paused(&PAUSE).lock().unwrap() {
        return Err("Better Lyrics sedang meminta jeda; sumber lain dicoba.".into());
    }
    let album: String = album.chars().take(512).collect();
    let seconds = if duration.is_finite() && duration > 0.0 { duration.round().min(86400.0) as u32 } else { 0 };
    let url = format!(
        "https://api.betterlyrics.org/getLyrics?s={}&a={}&al={}&d={seconds}",
        http::encode(title),
        http::encode(artist),
        http::encode(&album)
    );
    let response = http::get(&url).map_err(|_| "Better Lyrics belum dapat dihubungi; sumber lain dicoba.".to_string())?;
    match response.status {
        200 => {}
        401 | 403 => return Err("Better Lyrics belum menyediakan hasil ini tanpa akses tambahan; sumber lain dicoba.".into()),
        429 | 503 => {
            // shortcut: jeda tetap 60 detik, header Retry-After tidak dibaca.
            *paused(&PAUSE).lock().unwrap() = Instant::now() + Duration::from_secs(60);
            return Err("Better Lyrics sedang meminta jeda; sumber lain dicoba.".into());
        }
        204 | 404 => return Err("Romaji Better Lyrics belum tersedia; sumber lain dicoba.".into()),
        status => return Err(format!("Better Lyrics belum dapat diakses (HTTP {status}); sumber lain dicoba.")),
    }
    let json: Value = serde_json::from_slice(&response.body).map_err(|_| "Respons Better Lyrics belum bisa digunakan.".to_string())?;
    better_from_json(&json, title, artist, album, duration)
}

/// Respons Better Lyrics -> rekaman romaji, dan apakah cukup yakin untuk dipilih otomatis.
fn better_from_json(json: &Value, title: &str, artist: &str, album: String, duration: f64) -> Result<(Record, bool), String> {
    let ttml = json.get("ttml").and_then(Value::as_str).ok_or("Better Lyrics belum mengirim lirik berwaktu; sumber lain dicoba.")?;
    let lines = ttml_lines(ttml, duration, true);
    if lines.is_empty() {
        return Err("Romaji berwaktu Better Lyrics belum lengkap atau belum cocok; sumber lain dicoba.".into());
    }
    let score = json.get("score").and_then(Value::as_f64).filter(|s| (0.0..=1.0).contains(s));
    let record = Record {
        track: title.into(),
        artist: artist.into(),
        album,
        duration,
        lines: Some(lines),
        romanized: true,
        source: "Better Lyrics",
        ..Default::default()
    };
    // Tanpa score (hasil cache) atau score rendah: pilih manual, jangan tampil otomatis (bisa lagu lain).
    Ok((record, score.is_some_and(|s| s >= 0.90)))
}

// ---------------------------------------------------------------------------------------------
// Unison

fn unison(path: &str, body: Option<Value>) -> Result<Option<Value>, String> {
    static NEXT: OnceLock<Mutex<Instant>> = OnceLock::new();
    let mut next = paused(&NEXT).lock().unwrap(); // Satu permintaan sekaligus, jeda minimal 0,5 detik.
    let wait = next.saturating_duration_since(Instant::now());
    if wait > Duration::from_secs(2) {
        return Err(format!("Unison meminta jeda sekitar {} detik.", wait.as_secs() + 1));
    }
    thread::sleep(wait);
    let url = format!("https://unison.boidu.dev{path}");
    let response = match &body {
        Some(body) => http::post_json(&url, &body.to_string()),
        None => http::get(&url),
    };
    *next = Instant::now() + Duration::from_millis(500);
    let response = response.map_err(|_| "Unison belum dapat dihubungi.".to_string())?;
    match response.status {
        200..=299 => Ok(serde_json::from_slice(&response.body).ok()),
        404 if body.is_none() => Ok(None),
        429 | 503 => {
            *next = Instant::now() + Duration::from_secs(30); // shortcut: Retry-After tidak dibaca.
            Err("Unison sedang sibuk atau meminta jeda.".into())
        }
        status => Err(format!("Unison belum dapat memproses lirik (HTTP {status}).")),
    }
}

fn unison_items(document: Option<Value>) -> Vec<Value> {
    let Some(mut document) = document else { return Vec::new() };
    if document.get("success").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    match document["data"].take() {
        Value::Array(items) => items.into_iter().take(5).collect(),
        data @ Value::Object(_) => vec![data],
        _ => Vec::new(),
    }
}

fn unison_record(value: &Value, metadata_only: bool, prefer_romaji: bool) -> Option<Record> {
    let text = |name: &str, max: usize| value.get(name).and_then(Value::as_str).filter(|s| s.chars().count() <= max).unwrap_or("").to_string();
    let track = text("song", 512);
    if track.trim().is_empty() {
        return None;
    }
    let mut record = Record {
        track,
        artist: text("artist", 512),
        album: text("album", 512),
        language: text("language", 32),
        id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
        duration: value.get("duration").and_then(Value::as_f64).filter(|d| *d > 0.0 && *d <= 86400.0).unwrap_or(0.0),
        source: "Unison",
        ..Default::default()
    };
    if metadata_only {
        return Some(record);
    }
    let format = text("format", 16).to_ascii_lowercase();
    let lyrics = text("lyrics", if format == "ttml" { MAX_XML_CHARS } else { MAX_SOURCE_CHARS });
    if lyrics.is_empty() {
        return None;
    }
    match format.as_str() {
        "ttml" => {
            let mut lines = if prefer_romaji { ttml_lines(&lyrics, record.duration, true) } else { Vec::new() };
            record.romanized = !lines.is_empty();
            if lines.is_empty() {
                lines = ttml_lines(&lyrics, record.duration, false);
            }
            if lines.is_empty() {
                return None;
            }
            record.lines = Some(lines);
        }
        "lrc" => record.synced = Some(lyrics),
        "plain" => record.plain = Some(lyrics),
        _ => return None,
    }
    let lyrics = super::to_lyrics(&record)?;
    if record.language.to_ascii_lowercase().starts_with("ja-latn") && !has_japanese(&lyrics.text()) {
        record.romanized = true;
    }
    Some(record)
}

/// Lirik komunitas Unison: pencarian tepat dulu, lalu pencarian bebas (badan lirik diambil per ID, maks. 3).
pub(super) fn unison_find(title: &str, artist: &str, album: &str, duration: f64, prefer_romaji: bool) -> Result<Vec<Record>, String> {
    let mut records = Vec::new();
    if !artist.trim().is_empty() {
        let mut path = format!("/lyrics?song={}&artist={}", http::encode(title), http::encode(artist));
        if !album.is_empty() {
            path += &format!("&album={}", http::encode(album));
        }
        if duration > 0.0 && duration <= 86400.0 {
            path += &format!("&duration={}", (duration * 1000.0).round() / 1000.0);
        }
        records.extend(unison_items(unison(&path, None)?).iter().filter_map(|v| unison_record(v, false, prefer_romaji)));
        if !records.is_empty() {
            return Ok(records);
        }
    }
    let search = unison_items(unison(&format!("/lyrics/search?limit=5&q={}", http::encode(&format!("{title} {artist}"))), None)?);
    let metadata: Vec<Record> = search.iter().filter_map(|v| unison_record(v, true, prefer_romaji)).collect();
    for candidate in rank(metadata, title, artist, 0.0).into_iter().take(3) {
        if candidate.record.id <= 0 {
            continue;
        }
        let full = unison(&format!("/lyrics/{}", candidate.record.id), None)?;
        records.extend(unison_items(full).iter().filter_map(|v| unison_record(v, false, prefer_romaji)));
    }
    Ok(records)
}

/// Cadangan romaji lokal: baris Jepang yang tersisa diubah lewat Unison /translate.
/// Semua atau tidak sama sekali; waktu dan urutan baris tidak diubah.
pub(super) fn unison_romanize(lyrics: &mut Lyrics) -> Result<(), String> {
    let positions: Vec<usize> = (0..lyrics.lines.len()).filter(|&i| has_japanese(&lyrics.lines[i].text)).collect();
    if positions.is_empty() {
        return Ok(());
    }
    if positions.len() > 400 || positions.iter().any(|&i| lyrics.lines[i].text.chars().count() > 1000) {
        return Err("Teks terlalu besar untuk konversi otomatis.".into());
    }
    let mut romaji: Vec<String> = Vec::with_capacity(positions.len());
    let mut chars: usize = lyrics.lines.iter().map(|l| l.text.len() + 3).sum();
    let mut cursor = 0;
    for _ in 0..4 {
        if cursor == positions.len() {
            break;
        }
        let mut batch: Vec<&str> = Vec::new();
        let mut batch_chars = 0;
        while cursor < positions.len() && batch.len() < 100 {
            let line = lyrics.lines[positions[cursor]].text.as_str();
            if !batch.is_empty() && batch_chars + line.len() > 12_000 {
                break;
            }
            batch.push(line);
            batch_chars += line.len();
            cursor += 1;
        }
        let reply = unison("/translate", Some(json!({ "lines": batch, "to": "en", "from": "ja" })))?.unwrap_or_default();
        let lines = reply.get("lines").and_then(Value::as_array).filter(|l| l.len() == batch.len());
        let lines = lines.ok_or("Jumlah baris romaji tidak cocok; hasil konversi tidak dipakai.")?;
        let language = reply.get("detectedLang").and_then(Value::as_str).unwrap_or("");
        if !language.is_empty() && !language.eq_ignore_ascii_case("ja") {
            return Err("Bahasa hasil konversi bukan Jepang.".into());
        }
        for (line, original) in lines.iter().zip(&batch) {
            // Kolom translation adalah terjemahan Inggris, bukan bacaan romaji.
            let text = line.get("romanization").and_then(Value::as_str).unwrap_or("").trim();
            if text.is_empty() || text.len() > MAX_LINE_CHARS || has_japanese(text) || !text.chars().any(|c| c.is_ascii_alphabetic()) || text.contains(['\n', '\r']) {
                return Err("Romaji dari Unison belum lengkap.".into());
            }
            chars = (chars + text.len()).saturating_sub(original.len());
            if chars > MAX_SOURCE_CHARS {
                return Err("Hasil romaji terlalu besar.".into());
            }
            romaji.push(text.to_string());
        }
    }
    if cursor < positions.len() {
        return Err("Konversi melewati batas empat kelompok.".into());
    }
    for (i, text) in positions.into_iter().zip(romaji) {
        lyrics.lines[i].text = text;
    }
    lyrics.source.push_str(" · romaji Unison");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xml:lang="ja">
<head><metadata><iTunesMetadata><transliterations>
<transliteration xml:lang="ja-Latn"><text for="L1"><span begin="1.0" end="2.0">kimi</span> <span begin="2.0" end="3.5">no &amp; na</span></text>
<text for="L2">sora</text></transliteration></transliterations></iTunesMetadata></metadata></head>
<body><div><p begin="00:01.000" end="00:03.500" itunes:key="L1">君の &amp; 名</p>
<p begin="10s" end="12s" itunes:key="L2">空</p><p begin="12s" end="13s" itunes:key="L3">Hello<br/>world</p></div></body></tt>"#;

    #[test]
    fn ttml_romaji_follows_original_timing() {
        let lines = ttml_lines(TTML, 0.0, true);
        let texts: Vec<(f64, &str)> = lines.iter().map(|l| (l.seconds, l.text.as_str())).collect();
        assert_eq!(texts, vec![(1.0, "kimi no & na"), (3.5, ""), (10.0, "sora"), (12.0, "Hello world"), (13.0, "")]);
        let original = ttml_lines(TTML, 0.0, false);
        assert_eq!(original[0].text, "君の & 名");
    }

    #[test]
    fn ttml_rejects_missing_romaji_and_doctype() {
        let without = TTML.replace(r#"<text for="L2">sora</text>"#, "");
        assert!(ttml_lines(&without, 0.0, true).is_empty()); // Baris Jepang tanpa romaji.
        assert!(ttml_lines(&format!("<!DOCTYPE x>{TTML}"), 0.0, false).is_empty());
    }

    #[test]
    fn better_lyrics_automatic_only_with_high_score() {
        let sure = |score: Value| {
            let json = serde_json::json!({ "ttml": TTML, "score": score });
            better_from_json(&json, "Kimi", "Artis", String::new(), 0.0).unwrap().1
        };
        assert!(sure(Value::from(0.95)));
        assert!(!sure(Value::from(0.5))); // Skor rendah: bisa lagu lain, pilih manual.
        assert!(!sure(Value::Null)); // Tanpa skor (hasil cache): pilih manual.
        assert!(!sure(Value::from(1.5))); // Skor di luar 0..1 tidak dipercaya.
    }

    #[test]
    fn time_formats_parse() {
        assert_eq!(parse_time(Some("1500ms")), Some(1.5));
        assert_eq!(parse_time(Some("1:02:03.5")), Some(3723.5));
        assert_eq!(parse_time(Some("00:61")), None);
        assert_eq!(parse_time(Some("-1s")), None);
    }
}
