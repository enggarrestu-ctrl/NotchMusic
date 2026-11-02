using System.Globalization;
using System.Windows;
using System.Windows.Controls;

namespace NotchMusic;

// Halaman keempat: jadwal salat kota pilihan, dihitung offline (tanpa internet, tanpa izin lokasi).
// Saat masuk waktu salat, notch kecil menampilkan pengingat seperti notifikasi.
public partial class MainWindow
{
    private const int PrayerPageIndex = 3;
    private bool _prayerCityReady;

    private void InitializePrayerPage()
    {
        PrayerCity.ItemsSource = PrayerCities.All.Select(x => x.Name).ToList();
        PrayerCity.SelectedItem = PrayerCities.Find(_preferences.PrayerCity).Name;
        _prayerCityReady = true;
    }

    private void PrayerCity_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_prayerCityReady || PrayerCity.SelectedItem is not string name) return;
        _preferences.PrayerCity = name; SavePreferences();
        RenderPrayer();
    }

    private void PrayerCity_DropDownClosed(object sender, EventArgs e)
    {
        if (!_closed && !IsMouseOver) _collapseTimer.Start();
    }

    private void RenderPrayer()
    {
        if (_closed || !_preferences.PrayerEnabled) return;
        var city = PrayerCities.Find(_preferences.PrayerCity);
        var now = city.Now;
        var times = PrayerTimes.For(DateOnly.FromDateTime(now), city);
        var today = times.FirstOrDefault(x => x.Prays && x.Time > now);
        // Lewat Isya: berikutnya Subuh besok.
        var next = today ?? PrayerTimes.For(DateOnly.FromDateTime(now).AddDays(1), city)[0];
        PrayerList.ItemsSource = times.Select(x => new PrayerRow(x.Name, x.Time.ToString("HH.mm", CultureInfo.InvariantCulture),
            x == today, x.Time <= now)).ToList();
        PrayerNextLabel.Text = today is null ? "BESOK" : "BERIKUTNYA";
        PrayerNextName.Text = next.Name + " ";
        PrayerNextTime.Text = next.Time.ToString("HH.mm", CultureInfo.InvariantCulture);
        PrayerCountdown.Text = Countdown(next.Time - now) + " · " + city.Zone;
    }

    // "1 jam 5 menit lagi" / "59 menit lagi" / "kurang dari 1 menit": menit dibulatkan ke atas agar tidak terlambat.
    internal static string Countdown(TimeSpan left)
    {
        var minutes = (int)Math.Ceiling(Math.Max(0, left.TotalMinutes));
        if (minutes < 1) return "kurang dari 1 menit";
        var hours = minutes / 60; minutes %= 60;
        return hours == 0 ? $"{minutes} menit lagi" : minutes == 0 ? $"{hours} jam lagi" : $"{hours} jam {minutes} menit lagi";
    }

    // Dipanggil tiap pergantian menit oleh jam notch.
    private void CheckPrayerTime()
    {
        if (!_preferences.PrayerEnabled) return;
        var city = PrayerCities.Find(_preferences.PrayerCity);
        var now = city.Now;
        var due = PrayerTimes.For(DateOnly.FromDateTime(now), city)
            .FirstOrDefault(x => x.Prays && x.Time.Hour == now.Hour && x.Time.Minute == now.Minute);
        if (due is not null) ShowNotice($"Waktu {due.Name} · {due.Time:HH.mm} {city.Zone}");
        if (_expanded && _expandedPage == PrayerPageIndex) RenderPrayer();
    }
}

internal sealed record PrayerRow(string Name, string Time, bool IsNext, bool IsPast);
internal sealed record PrayerTime(string Name, DateTime Time, bool Prays);
internal sealed record PrayerCity(string Name, double Latitude, double Longitude, int UtcOffset)
{
    public string Zone => UtcOffset switch { 7 => "WIB", 8 => "WITA", 9 => "WIT", _ => $"UTC+{UtcOffset}" };
    public DateTime Now => DateTime.UtcNow.AddHours(UtcOffset);
}

// Rumus posisi matahari (praytimes.org) dengan kriteria Kemenag RI: Subuh 20°, Isya 18°, Ashar bayangan 1x,
// ihtiyat +2 menit (Dzuhur +3, Terbit -3), dicocokkan dengan jadwal Kemenag (api.myquran.com) Jakarta, Surabaya,
// Makassar: selisih maksimal 1 menit. Ketinggian tempat diabaikan.
internal static class PrayerTimes
{
    public static List<PrayerTime> For(DateOnly date, PrayerCity city)
    {
        // Posisi matahari sekitar tengah hari setempat; cukup untuk ketelitian menit.
        var jd = date.ToDateTime(TimeOnly.MinValue).ToOADate() + 2415018.5 - city.Longitude / 360 + 0.5;
        var d = jd - 2451545.0;
        var g = Rad(357.529 + 0.98560028 * d);
        var q = Wrap(280.459 + 0.98564736 * d, 360);
        var l = Rad(q + 1.915 * Math.Sin(g) + 0.020 * Math.Sin(2 * g));
        var e = Rad(23.439 - 0.00000036 * d);
        var declination = Math.Asin(Math.Sin(e) * Math.Sin(l));
        var rightAscension = Deg(Math.Atan2(Math.Cos(e) * Math.Sin(l), Math.Cos(l))) / 15;
        var equation = q / 15 - Wrap(rightAscension, 24);
        equation -= Math.Round(equation / 24) * 24;
        var lat = Rad(city.Latitude);
        var noon = 12 + city.UtcOffset - city.Longitude / 15 - equation;
        double Hours(double sunAltitude) =>
            Deg(Math.Acos(Math.Clamp((Math.Sin(Rad(sunAltitude)) - Math.Sin(declination) * Math.Sin(lat))
                / (Math.Cos(declination) * Math.Cos(lat)), -1, 1))) / 15;
        var asrAltitude = Deg(Math.Atan(1 / (1 + Math.Tan(Math.Abs(lat - declination)))));
        DateTime At(double hours, int minutes) => date.ToDateTime(TimeOnly.MinValue)
            .AddMinutes(Math.Ceiling(hours * 60) + minutes); // Dibulatkan ke atas seperti jadwal resmi.
        return
        [
            new("Subuh", At(noon - Hours(-20), 2), true),
            new("Terbit", At(noon - Hours(-0.833), -3), false),
            new("Dzuhur", At(noon, 3), true),
            new("Ashar", At(noon + Hours(asrAltitude), 2), true),
            new("Maghrib", At(noon + Hours(-0.833), 2), true),
            new("Isya", At(noon + Hours(-18), 2), true),
        ];
    }

    private static double Rad(double degrees) => degrees * Math.PI / 180;
    private static double Deg(double radians) => radians * 180 / Math.PI;
    private static double Wrap(double value, double range) => value - range * Math.Floor(value / range);
}

internal static class PrayerCities
{
    // Ibu kota provinsi dan kota besar Indonesia. shortcut: daftar tetap, tambah baris bila kotamu belum ada.
    public static readonly PrayerCity[] All = new PrayerCity[]
    {
        new("Ambon", -3.695, 128.181, 9), new("Balikpapan", -1.265, 116.831, 8), new("Banda Aceh", 5.548, 95.323, 7),
        new("Bandar Lampung", -5.397, 105.266, 7), new("Bandung", -6.917, 107.619, 7), new("Banjarmasin", -3.317, 114.590, 8),
        new("Batam", 1.045, 104.031, 7), new("Bekasi", -6.238, 106.976, 7), new("Bengkulu", -3.800, 102.266, 7),
        new("Bogor", -6.595, 106.816, 7), new("Cirebon", -6.732, 108.552, 7), new("Denpasar", -8.650, 115.217, 8),
        new("Depok", -6.402, 106.794, 7), new("Gorontalo", 0.543, 123.058, 8), new("Jakarta", -6.208, 106.846, 7),
        new("Jambi", -1.610, 103.613, 7), new("Jayapura", -2.533, 140.717, 9), new("Jember", -8.172, 113.700, 7),
        new("Kediri", -7.817, 112.012, 7), new("Kendari", -3.972, 122.515, 8), new("Kupang", -10.178, 123.607, 8),
        new("Makassar", -5.148, 119.432, 8), new("Malang", -7.983, 112.621, 7), new("Mamuju", -2.675, 118.889, 8),
        new("Manado", 1.474, 124.842, 8), new("Manokwari", -0.862, 134.064, 9), new("Mataram", -8.583, 116.117, 8),
        new("Medan", 3.595, 98.672, 7), new("Merauke", -8.493, 140.401, 9), new("Padang", -0.947, 100.417, 7),
        new("Palangka Raya", -2.208, 113.917, 7), new("Palembang", -2.976, 104.775, 7), new("Palu", -0.895, 119.859, 8),
        new("Pangkalpinang", -2.129, 106.113, 7), new("Pekanbaru", 0.507, 101.448, 7), new("Pontianak", -0.026, 109.342, 7),
        new("Purwokerto", -7.424, 109.239, 7), new("Samarinda", -0.502, 117.154, 8), new("Semarang", -6.967, 110.417, 7),
        new("Serang", -6.120, 106.150, 7), new("Sorong", -0.877, 131.255, 9), new("Surabaya", -7.250, 112.768, 7),
        new("Surakarta (Solo)", -7.576, 110.824, 7), new("Tangerang", -6.178, 106.630, 7), new("Tanjung Selor", 2.837, 117.367, 8),
        new("Tanjungpinang", 0.918, 104.446, 7), new("Tasikmalaya", -7.327, 108.220, 7), new("Ternate", 0.790, 127.384, 9),
        new("Yogyakarta", -7.797, 110.370, 7),
    };

    public static PrayerCity Find(string? name)
        => All.FirstOrDefault(x => string.Equals(x.Name, name, StringComparison.OrdinalIgnoreCase)) ?? All.First(x => x.Name == "Jakarta");
}
