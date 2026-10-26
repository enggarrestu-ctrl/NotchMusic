using System.Diagnostics;
using System.Globalization;
using System.Net.NetworkInformation;
using System.Windows;
using System.Windows.Threading;

namespace NotchMusic;

// Kecepatan internet di notch kecil: selisih total byte semua adapter aktif per detik.
// Hanya berjalan saat jam terlihat; lirik, notifikasi, panel terbuka, atau tray = berhenti.
public partial class MainWindow
{
    private readonly DispatcherTimer _speedTimer = new() { Interval = TimeSpan.FromSeconds(1) };
    private long _speedDown, _speedUp, _speedStamp;

    private void UpdateSpeedTimer()
    {
        var run = !_closed && !_minimizedToTray && !_expanded && CompactIdle.Visibility == Visibility.Visible;
        if (run == _speedTimer.IsEnabled) return;
        if (!run) { _speedTimer.Stop(); return; }
        _speedStamp = 0; // Sampel pertama hanya mengambil titik awal.
        SampleSpeed();
        _speedTimer.Start();
    }

    private bool _speedBusy;

    // Membaca adapter jaringan butuh ~15 ms: dikerjakan di thread latar agar animasi notch tidak tersendat.
    private async void SampleSpeed()
    {
        if (_speedBusy) return;
        _speedBusy = true;
        (long Down, long Up, long Now)? sample;
        try { sample = await Task.Run(() => NetworkTotals(out var d, out var u) ? (d, u, Stopwatch.GetTimestamp()) : ((long, long, long)?)null); }
        finally { _speedBusy = false; }
        if (sample is not var (down, up, now) || _closed) return;
        if (_speedStamp > 0)
        {
            var seconds = Stopwatch.GetElapsedTime(_speedStamp, now).TotalSeconds;
            if (seconds > 0.2)
            {
                // Adapter putus/tersambung bisa membuat total turun: anggap 0, bukan angka negatif.
                SpeedDown.Text = "↓ " + FormatSpeed(Math.Max(0, down - _speedDown) / seconds);
                SpeedUp.Text = "↑ " + FormatSpeed(Math.Max(0, up - _speedUp) / seconds);
            }
        }
        _speedDown = down; _speedUp = up; _speedStamp = now;
    }

    private static bool NetworkTotals(out long down, out long up)
    {
        down = up = 0;
        try
        {
            foreach (var adapter in NetworkInterface.GetAllNetworkInterfaces())
            {
                if (adapter.OperationalStatus != OperationalStatus.Up
                    || adapter.NetworkInterfaceType is NetworkInterfaceType.Loopback or NetworkInterfaceType.Tunnel) continue;
                var stats = adapter.GetIPStatistics();
                down += stats.BytesReceived; up += stats.BytesSent;
            }
            return true;
        }
        catch (NetworkInformationException ex) { Debug.WriteLine(ex); return false; }
    }

    internal static string FormatSpeed(double bytesPerSecond)
    {
        var kb = bytesPerSecond / 1024;
        if (kb < 1000) return Math.Round(kb).ToString("0", CultureInfo.InvariantCulture) + " KB/s";
        var mb = kb / 1024;
        return mb.ToString(mb < 100 ? "0.0" : "0", CultureInfo.InvariantCulture) + " MB/s";
    }
}
