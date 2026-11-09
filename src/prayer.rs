// Jadwal salat offline (port PrayerFeature.cs / PrayerSky.cs NotchMusic): posisi matahari
// (praytimes.org) dengan kriteria Kemenag RI, daftar kota, hitung mundur, dan tata letak lintasan matahari.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct City {
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub utc: i32,
}

impl City {
    pub fn zone(&self) -> &'static str {
        match self.utc {
            7 => "WIB",
            8 => "WITA",
            9 => "WIT",
            _ => "",
        }
    }

    /// Hari (sejak 1970-01-01) dan menit sejak tengah malam di zona waktu kota ini.
    pub fn now(&self) -> (i64, f64) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
            + self.utc as f64 * 3600.0;
        let day = (secs / 86_400.0).floor();
        (day as i64, (secs - day * 86_400.0) / 60.0)
    }
}

const fn city(name: &'static str, lat: f64, lon: f64, utc: i32) -> City {
    City { name, lat, lon, utc }
}

// Ibu kota provinsi dan kota besar Indonesia. shortcut: daftar tetap, tambah baris bila kotamu belum ada.
pub const CITIES: [City; 49] = [
    city("Ambon", -3.695, 128.181, 9), city("Balikpapan", -1.265, 116.831, 8), city("Banda Aceh", 5.548, 95.323, 7),
    city("Bandar Lampung", -5.397, 105.266, 7), city("Bandung", -6.917, 107.619, 7), city("Banjarmasin", -3.317, 114.590, 8),
    city("Batam", 1.045, 104.031, 7), city("Bekasi", -6.238, 106.976, 7), city("Bengkulu", -3.800, 102.266, 7),
    city("Bogor", -6.595, 106.816, 7), city("Cirebon", -6.732, 108.552, 7), city("Denpasar", -8.650, 115.217, 8),
    city("Depok", -6.402, 106.794, 7), city("Gorontalo", 0.543, 123.058, 8), city("Jakarta", -6.208, 106.846, 7),
    city("Jambi", -1.610, 103.613, 7), city("Jayapura", -2.533, 140.717, 9), city("Jember", -8.172, 113.700, 7),
    city("Kediri", -7.817, 112.012, 7), city("Kendari", -3.972, 122.515, 8), city("Kupang", -10.178, 123.607, 8),
    city("Makassar", -5.148, 119.432, 8), city("Malang", -7.983, 112.621, 7), city("Mamuju", -2.675, 118.889, 8),
    city("Manado", 1.474, 124.842, 8), city("Manokwari", -0.862, 134.064, 9), city("Mataram", -8.583, 116.117, 8),
    city("Medan", 3.595, 98.672, 7), city("Merauke", -8.493, 140.401, 9), city("Padang", -0.947, 100.417, 7),
    city("Palangka Raya", -2.208, 113.917, 7), city("Palembang", -2.976, 104.775, 7), city("Palu", -0.895, 119.859, 8),
    city("Pangkalpinang", -2.129, 106.113, 7), city("Pekanbaru", 0.507, 101.448, 7), city("Pontianak", -0.026, 109.342, 7),
    city("Purwokerto", -7.424, 109.239, 7), city("Samarinda", -0.502, 117.154, 8), city("Semarang", -6.967, 110.417, 7),
    city("Serang", -6.120, 106.150, 7), city("Sorong", -0.877, 131.255, 9), city("Surabaya", -7.250, 112.768, 7),
    city("Surakarta (Solo)", -7.576, 110.824, 7), city("Tangerang", -6.178, 106.630, 7), city("Tanjung Selor", 2.837, 117.367, 8),
    city("Tanjungpinang", 0.918, 104.446, 7), city("Tasikmalaya", -7.327, 108.220, 7), city("Ternate", 0.790, 127.384, 9),
    city("Yogyakarta", -7.797, 110.370, 7),
];

pub const JAKARTA: usize = 14;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PrayerTime {
    pub name: &'static str,
    /// Menit sejak tengah malam waktu kota.
    pub minutes: f64,
    /// Terbit bukan waktu salat (tanpa pengingat).
    pub prays: bool,
}

impl PrayerTime {
    pub fn label(&self) -> String {
        let m = self.minutes.round() as i64;
        format!("{:02}.{:02}", m.div_euclid(60) % 24, m.rem_euclid(60))
    }
}

/// Subuh 20°, Isya 18°, Ashar bayangan 1x; ihtiyat +2 menit (Dzuhur +3, Terbit -3), dibulatkan ke atas.
/// Dicocokkan dengan jadwal Kemenag (api.myquran.com) Jakarta, Surabaya, Makassar: selisih maksimal 1 menit.
pub fn times(day: i64, city: &City) -> [PrayerTime; 6] {
    let jd = day as f64 + 2_440_587.5 - city.lon / 360.0 + 0.5;
    let d = jd - 2_451_545.0;
    let g = (357.529 + 0.985_600_28 * d).to_radians();
    let q = (280.459 + 0.985_647_36 * d).rem_euclid(360.0);
    let l = (q + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let e = (23.439 - 0.000_000_36 * d).to_radians();
    let declination = (e.sin() * l.sin()).asin();
    let ascension = (e.cos() * l.sin()).atan2(l.cos()).to_degrees() / 15.0;
    let mut equation = q / 15.0 - ascension.rem_euclid(24.0);
    equation -= (equation / 24.0).round() * 24.0;
    let lat = city.lat.to_radians();
    let noon = 12.0 + city.utc as f64 - city.lon / 15.0 - equation;
    let hours = |altitude: f64| {
        ((altitude.to_radians().sin() - declination.sin() * lat.sin()) / (declination.cos() * lat.cos()))
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
            / 15.0
    };
    let asr = (1.0 / (1.0 + (lat - declination).abs().tan())).atan().to_degrees();
    let at = |h: f64, extra: f64| (h * 60.0).ceil() + extra;
    [
        PrayerTime { name: "Subuh", minutes: at(noon - hours(-20.0), 2.0), prays: true },
        PrayerTime { name: "Terbit", minutes: at(noon - hours(-0.833), -3.0), prays: false },
        PrayerTime { name: "Dzuhur", minutes: at(noon, 3.0), prays: true },
        PrayerTime { name: "Ashar", minutes: at(noon + hours(asr), 2.0), prays: true },
        PrayerTime { name: "Maghrib", minutes: at(noon + hours(-0.833), 2.0), prays: true },
        PrayerTime { name: "Isya", minutes: at(noon + hours(-18.0), 2.0), prays: true },
    ]
}

/// "1 jam 5 menit lagi" / "59 menit lagi" / "kurang dari 1 menit": dibulatkan ke atas agar tidak terlambat.
pub fn countdown(minutes_left: f64) -> String {
    let minutes = minutes_left.max(0.0).ceil() as i64;
    if minutes < 1 {
        return "kurang dari 1 menit".into();
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, m) => format!("{m} menit lagi"),
        (h, 0) => format!("{h} jam lagi"),
        (h, m) => format!("{h} jam {m} menit lagi"),
    }
}

/// Waktu salat berikutnya: (index hari ini, atau None = Subuh besok), waktunya, dan menit tersisa.
pub fn next(today: &[PrayerTime; 6], tomorrow_subuh: PrayerTime, now: f64) -> (Option<usize>, PrayerTime, f64) {
    match today.iter().position(|p| p.prays && p.minutes > now) {
        Some(i) => (Some(i), today[i], today[i].minutes - now),
        None => (None, tomorrow_subuh, tomorrow_subuh.minutes + 1440.0 - now),
    }
}

/// Lintasan matahari: busur Terbit -> Dzuhur (puncak) -> Maghrib; Subuh/Isya di cakrawala.
/// Waktu dalam menit sejak tengah malam; hasil dalam DIP relatif ke kiri-atas kanvas.
pub struct Sky<'a> {
    pub times: &'a [PrayerTime; 6],
    pub width: f32,
    pub height: f32,
}

impl Sky<'_> {
    const EDGE: f32 = 26.0;
    const DAWN: f32 = 76.0;

    pub fn horizon(&self) -> f32 {
        self.height - 32.0
    }

    fn peak(&self) -> f32 {
        self.horizon() - 8.0
    }

    pub fn x(&self, t: f64) -> f32 {
        let [subuh, rise, _, _, set, isya] = self.times.map(|p| p.minutes);
        let lerp = |a: f32, b: f32, from: f64, to: f64| a + (b - a) * ((t - from) / (to - from).max(1.0)).clamp(0.0, 1.0) as f32;
        let w = self.width;
        if t <= subuh {
            (Self::EDGE - ((subuh - t) / 30.0) as f32).max(6.0)
        } else if t <= rise {
            lerp(Self::EDGE, Self::DAWN, subuh, rise)
        } else if t <= set {
            lerp(Self::DAWN, w - Self::DAWN, rise, set)
        } else if t <= isya {
            lerp(w - Self::DAWN, w - Self::EDGE, set, isya)
        } else {
            (w - Self::EDGE + ((t - isya) / 30.0) as f32).min(w - 6.0)
        }
    }

    pub fn y(&self, t: f64) -> f32 {
        let (rise, set) = (self.times[1].minutes, self.times[4].minutes);
        if t <= rise || t >= set {
            return self.horizon();
        }
        let f = ((t - rise) / (set - rise)) as f32;
        self.horizon() - self.peak() * (std::f32::consts::PI * f).sin()
    }

    pub fn is_day(&self, t: f64) -> bool {
        t > self.times[1].minutes && t < self.times[4].minutes
    }
}

/// Hari sejak 1970-01-01 untuk tanggal kalender (algoritma days_from_civil, Howard Hinnant). Dipakai tes.
#[cfg(test)]
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> &'static City {
        CITIES.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn matches_kemenag_within_one_minute() {
        // Jadwal Kemenag 2 November 2026 dari api.myquran.com.
        let day = days_from_civil(2026, 11, 2);
        for (name, official) in [
            ("Jakarta", ["04:08", "05:23", "11:40", "14:55", "17:49", "19:01"]),
            ("Surabaya", ["03:43", "04:58", "11:16", "14:31", "17:27", "18:39"]),
            ("Makassar", ["04:19", "05:34", "11:49", "15:06", "17:58", "19:09"]),
        ] {
            let computed = times(day, find(name));
            for (p, o) in computed.iter().zip(official) {
                let (h, m) = o.split_once(':').unwrap();
                let expected = h.parse::<f64>().unwrap() * 60.0 + m.parse::<f64>().unwrap();
                assert!((p.minutes - expected).abs() <= 1.0, "{name} {} {} vs {o}", p.name, p.label());
            }
        }
        assert_eq!(CITIES[JAKARTA].name, "Jakarta");
    }

    #[test]
    fn countdown_reads_naturally() {
        assert_eq!(countdown(58.2), "59 menit lagi");
        assert_eq!(countdown(59.5), "1 jam lagi");
        assert_eq!(countdown(65.0), "1 jam 5 menit lagi");
        assert_eq!(countdown(0.0), "kurang dari 1 menit");
    }

    #[test]
    fn sun_path_rises_to_dzuhur_inside_canvas() {
        let t = times(days_from_civil(2026, 11, 2), find("Jakarta"));
        let sky = Sky { times: &t, width: 388.0, height: 86.0 };
        let xs: Vec<f32> = t.iter().map(|p| sky.x(p.minutes)).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]));
        assert!(sky.y(t[2].minutes) < sky.y(t[3].minutes) && sky.y(t[2].minutes) >= 0.0);
        assert_eq!(sky.y(t[0].minutes), sky.horizon());
        assert!((0.0..=388.0).contains(&sky.x(t[0].minutes - 360.0)));
    }

    #[test]
    fn next_prayer_wraps_to_tomorrow() {
        let day = days_from_civil(2026, 11, 2);
        let today = times(day, find("Jakarta"));
        let tomorrow = times(day + 1, find("Jakarta"))[0];
        let (index, p, _) = next(&today, tomorrow, 12.0 * 60.0);
        assert_eq!((index, p.name), (Some(3), "Ashar"));
        let (index, p, left) = next(&today, tomorrow, 23.0 * 60.0);
        assert_eq!((index, p.name), (None, "Subuh"));
        assert!(left > 0.0 && left < 6.0 * 60.0);
    }
}
