# NotchRust

Widget *notch* di bagian atas layar Windows, port Rust dari [NotchMusic](https://github.com/enggarrestu-ctrl/NotchMusic) (WPF).
Saat diam, notch menampilkan logo, jam, dan kecepatan internet. Saat di-hover, notch terbuka menjadi panel musik.

Dibuat dengan Win32 + Direct2D lewat crate [`windows`](https://crates.io/crates/windows), tanpa runtime .NET.

## Fitur (MVP)

- Lagu yang sedang diputar dari pemutar apa pun yang terhubung ke Windows (Spotify, YouTube Music, browser).
- Cover album, judul, artis, sumber, progres lagu, putar/jeda, sebelumnya/berikutnya, dan *seek* (klik garis progres).
- Garis progres tipis di tepi bawah notch kecil.
- Kecepatan internet ↓/↑.
- Tampilan mengikuti NotchMusic: bentuk notch menempel dengan sudut atas melengkung, logo N, enam tema bawaan
  (pertama kali jalan memakai tema yang sedang dipakai NotchMusic), tombol pin, tema, dan minimalkan ke tray.
- Selalu di atas, tidak muncul di Alt+Tab, ikon tray, dan opsi jalan otomatis saat Windows mulai (menu klik kanan).

**Lirik**
- Lirik berwaktu dari [LRCLIB](https://lrclib.net); lagu asli dari judul video YouTube dikenali lewat [Deezer](https://www.deezer.com).
- Kolom lirik (baris sebelumnya, sekarang, berikutnya) di panel terbuka; notch kecil menampilkan baris lirik + equalizer saat lagu diputar.
- Lagu Jepang tampil dalam romaji (penganalisis fonetik Jepang bawaan Windows).
- Lirik disimpan di `%LocalAppData%\NotchRust\lyrics`, jadi lagu yang sama tidak dicari ulang. Menu klik kanan: **Cari ulang lirik** dan **Lirik offline saja** (tanpa permintaan internet).
- Jendela **Lirik** (tombol Aa atau menu klik kanan), tampilannya sama dengan NotchMusic: kaca acrylic berwarna tema,
  jenis lirik (Otomatis/Lirik asli/Romaji) + Cari ulang, daftar "Lirik yang dipakai", koreksi waktu ±0,5 detik,
  bagian "Lainnya" (judul/artis untuk pencarian, simpan `.lrc`/`.txt`, muat file, Lirik tersimpan…), dan teks lirik lengkap.

**Clipboard** (halaman kedua, panah kiri/kanan)
- Riwayat teks yang disalin selama aplikasi berjalan, maksimal 64 item; tidak pernah disimpan ke disk.
- Pencarian, salin ulang, hapus, Jeda/Lanjut, dan Reset. Isi yang ditandai pengelola sandi tidak dicatat.

Belum di-port: romaji dari Better Lyrics/Unison, editor tema kustom, notifikasi,
kunci game, dan memindah notch. Halaman peluncur game sengaja tidak di-port.

## Build

Butuh [Rust](https://rustup.rs) stable.

```bash
cargo build --release
```

Hasil: `target/release/NotchRust.exe`. Taruh `NotchMusic_Tray_Mint.ico` di samping exe untuk ikon tray.

GitHub Actions (`.github/workflows/build.yml`) juga membangun exe di setiap push ke `main`; unduh dari artifact `NotchRust-windows-x64`.

## Struktur

| File | Isi |
|---|---|
| `src/main.rs` | Jendela notch, animasi, input mouse, menu, tray, autostart |
| `src/draw.rs` | Gambar notch dengan Direct2D/DirectWrite, cover dengan WIC |
| `src/media.rs` | Windows Media Session: info lagu dan kontrol |
| `src/net.rs` | Kecepatan internet |

## Lisensi

MIT
