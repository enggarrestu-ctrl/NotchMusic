# NotchMusic

Widget *notch* di bagian atas layar Windows, mirip notch MacBook. Saat diam, notch hanya menampilkan logo, jam, dan kecepatan internet. Saat di-hover, notch terbuka menjadi panel musik, clipboard, dan peluncur game.

Dibuat dengan WPF (.NET 10). Ringan: sekitar 25–50 MB RAM, dan tetap jalan di PC kantor tanpa GPU.

## Fitur

**Musik**
- Membaca lagu yang sedang diputar dari pemutar apa pun yang terhubung ke Windows (YouTube Music, YouTube di browser, Spotify, dan lain-lain).
- Kontrol putar/jeda, lagu sebelumnya/berikutnya, dan *seek*.
- Cover album HD.
- Garis progres di tepi notch yang bergerak sesuai lagu.

**Lirik**
- Lirik berwaktu yang bergulir.
- Lagu Jepang tampil dalam romaji. Lagu Inggris dan Indonesia tampil apa adanya.
- Mengenali lagu asli dari judul video YouTube yang berantakan.
- Lirik disimpan di PC, jadi lagu yang sama tidak dicari ulang. Bisa juga memuat file `.lrc`.
- Mode **lirik offline** (tanpa internet) dari menu klik kanan.

**Clipboard**
- Riwayat teks yang disalin selama aplikasi berjalan, maksimal 64 item.
- Pencarian, salin ulang, jeda, dan reset.
- Riwayat hilang saat aplikasi ditutup, dan tidak pernah disimpan ke disk.

**Game**
- Peluncur game sekali klik: hasil scan game Steam terakhir dimainkan, plus shortcut tambahanmu sendiri.
- Urutan diatur dengan seret, game dihapus lewat klik kanan.

**Lainnya**
- Notifikasi WhatsApp dan browser tampil singkat di notch. Notifikasi tidak ditampilkan saat presentasi atau saat ada aplikasi layar penuh.
- Pemberitahuan controller tersambung/terputus.
- Tema bawaan (termasuk **Hitam Legam**) dan editor tema kustom.
- **Kunci game:** saat game layar penuh terbuka, notch tidak bisa diklik. Klik kiri dua kali untuk membuka kuncinya.
- Bisa menempel di atas layar atau dipindah bebas.
- Selalu di atas, tidak muncul di Alt+Tab, punya ikon tray, dan bisa jalan otomatis saat Windows mulai.

## Kebutuhan

- Windows 11 (64-bit).
- Untuk menampilkan notifikasi, izinkan **akses notifikasi** saat diminta pertama kali.

## Build

Butuh [.NET SDK 10](https://dotnet.microsoft.com/download).

```bash
dotnet build NotchMusic/NotchMusic.csproj
```

```bash
dotnet test NotchMusic.Tests
```

Publish *self-contained*, sehingga PC tujuan tidak perlu memasang .NET. Folder tujuan diatur di `NotchMusic/Properties/PublishProfiles/FolderProfile.pubxml`:

```bash
dotnet publish NotchMusic/NotchMusic.csproj -p:PublishProfile=FolderProfile
```

## Privasi

- Tidak ada analytics, pelacakan, atau akun.
- Untuk mencari lirik, judul dan artis lagu dikirim ke [LRCLIB](https://lrclib.net), [Deezer](https://www.deezer.com), dan Better Lyrics (`api.betterlyrics.org`). Lirik Jepang bisa dikirim ke Unison (`unison.boidu.dev`) untuk diubah ke romaji. Nyalakan **Lirik offline saja** di menu klik kanan untuk mematikan semua permintaan ini.
- Pengaturan dan cache lirik disimpan di `%LocalAppData%\NotchMusic`.

## Struktur

| File | Isi |
|---|---|
| `NotchMusic/MainWindow.xaml(.cs)` | Notch, musik, tema, tray, dan fungsi Windows |
| `NotchMusic/LyricsFeature.cs` | Pencarian lirik, romaji, cache, dan file `.lrc` |
| `NotchMusic/ClipboardFeature.cs` | Riwayat clipboard dan perpindahan halaman |
| `NotchMusic/GameFeature.cs` | Peluncur game dan pembaca data Steam |
| `NotchMusic/NetSpeedFeature.cs` | Kecepatan internet |
| `NotchMusic.Tests/` | Tes xUnit |

## Lisensi

[MIT](LICENSE) © 2026 enggarrestu-ctrl
