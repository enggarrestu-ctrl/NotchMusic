# NotchRust

Widget *notch* di bagian atas layar Windows, port Rust dari [NotchMusic](https://github.com/enggarrestu-ctrl/NotchMusic) (WPF).
Saat diam, notch menampilkan logo, jam, dan kecepatan internet. Saat di-hover, notch terbuka menjadi panel musik.

Dibuat dengan Win32 + Direct2D + DirectComposition lewat crate [`windows`](https://crates.io/crates/windows), tanpa runtime .NET. Notch ditampilkan lewat swap chain DirectComposition (langsung dari GPU, tersinkron refresh monitor; WARP bila tidak ada GPU), dan animasi digerakkan per vsync.

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

**Game** (halaman ketiga)
- Peluncur game sekali klik: tombol Scan menambah game Steam terakhir dimainkan, tombol + menambah shortcut/aplikasi sendiri (ikon diambil dari shortcut-nya).
- Seret ikon untuk mengubah urutan (garis penanda tempat taruh), klik kanan untuk menghapus. Pertama kali jalan memakai daftar game NotchMusic.

**Jadwal Salat** (halaman keempat)
- Enam waktu salat hari ini untuk 49 kota (pil kota di kanan atas), dihitung offline dengan kriteria Kemenag RI; selisih dengan jadwal resmi maksimal 1 menit.
- Kartu waktu berikutnya dengan hitung mundur, lintasan matahari (busur siang, matahari/bulan saat ini), dan pengingat di notch kecil saat masuk waktu salat.

**Clipboard** (halaman kedua, panah kiri/kanan)
- Riwayat teks yang disalin selama aplikasi berjalan, maksimal 64 item; tidak pernah disimpan ke disk.
- Pencarian, salin ulang, hapus, Jeda/Lanjut, dan Reset. Isi yang ditandai pengelola sandi tidak dicatat.

**Notifikasi**: notifikasi WhatsApp dan browser tampil 5 detik di notch kecil (tidak saat presentasi/aplikasi layar penuh), plus pemberitahuan controller tersambung/terputus. Perlu izin akses notifikasi Windows.

**Pindah notch**: seret dari area kosong (notch kembali kecil selama diseret); dilepas dekat tepi atas = menempel dengan sudut atas melengkung, di tempat lain = mengambang dengan bentuk bulat. Posisi tersimpan; menu klik kanan: Reset posisi.

**Kunci game**: saat ada game/aplikasi layar penuh di monitor notch, notch tidak terbuka saat di-hover dan menu klik kanan tidak muncul; klik kiri dua kali untuk membukanya.

**Fitur aktif** (menu klik kanan): Halaman Clipboard, Halaman Jadwal Salat (dan pengingat), serta Notifikasi dan kecepatan internet bisa dimatikan. Halaman yang dimatikan dilewati, panah hilang bila tinggal satu halaman, dan riwayat clipboard dibuang saat fiturnya dimatikan.

Belum di-port: editor tema kustom dan romaji dari Better Lyrics/Unison.

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
| `src/draw.rs` | Gambar notch dengan Direct2D/DirectWrite ke swap chain DirectComposition, cover dengan WIC |
| `src/media.rs` | Windows Media Session: info lagu dan kontrol |
| `src/net.rs` | Kecepatan internet |
| `src/notify.rs` | Notifikasi WhatsApp/browser dan controller |
| `src/prayer.rs` | Jadwal salat, daftar kota, hitung mundur, tata letak lintasan matahari |

## Lisensi

MIT
