using System.IO;
using NotchMusic;
using Xunit;

namespace NotchMusic.Tests;

public class CoreTests
{
    [Fact]
    public void PreferencesMergeOldCustomGamesAndDropBadEntries()
    {
        var path = Path.Combine(Path.GetTempPath(), $"notch-test-{Guid.NewGuid():N}.json");
        try
        {
            File.WriteAllText(path, """
                {"GameOrder":["570","570","not a path","12345678901"],
                 "CustomGames":["C:\\Games\\A.lnk","relative.lnk"],
                 "CustomColors":["#000000"], "CenterX": 1e300, "HasPosition": true}
                """);
            var p = NotchPreferences.Load(path);
            Assert.Equal(["570", @"C:\Games\A.lnk"], p.GameOrder);
            Assert.Null(p.CustomGames);
            Assert.Null(p.CustomColors);
            Assert.False(p.HasPosition);
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void PreferencesBrokenFileFallsBackToDefaults()
    {
        var path = Path.Combine(Path.GetTempPath(), $"notch-test-{Guid.NewGuid():N}.json");
        try
        {
            File.WriteAllText(path, "{ rusak");
            Assert.True(NotchPreferences.Load(path).PinOnTop);
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void LrcParsesTimesAndOffset()
    {
        var doc = LyricDocument.From(new LrcRecord { SyncedLyrics = "[offset:500]\n[00:01.00]satu\n[01:02.50]dua\nbukan baris" });
        Assert.True(doc.IsSynced);
        Assert.Equal(["satu", "dua"], doc.Lines.Select(x => x.Text));
        Assert.Equal(0.5, doc.Lines[0].Seconds, 3);
        Assert.Equal(62.0, doc.Lines[1].Seconds, 3);
    }

    [Fact]
    public void PlacementDocksNearTopAndStaysOnScreen()
    {
        var screen = new DesktopNative.NativeRect { Left = 0, Top = 0, Right = 1920, Bottom = 1080 };
        Assert.Equal((860, 0, true), NotchPlacement.Resolve(960, 5, 0.5, 0.5, 200, 10, screen, 1, false));
        var floating = NotchPlacement.Resolve(5000, 500, 0.5, 0.5, 200, 40, screen, 1, true);
        Assert.Equal((1720, 480, false), floating);
    }

    [Fact]
    public void ClipboardKeepsNewestAndRespectsLimits()
    {
        var buffer = new ClipboardHistoryBuffer();
        Assert.False(buffer.Add("   ", DateTimeOffset.Now));
        Assert.False(buffer.Add(new string('x', ClipboardHistoryBuffer.MaxTextCharacters + 1), DateTimeOffset.Now));
        for (var i = 0; i < ClipboardHistoryBuffer.MaxItems + 5; i++) buffer.Add("teks " + i, DateTimeOffset.Now);
        buffer.Add("teks 10", DateTimeOffset.Now); // Duplikat pindah ke paling atas, tidak dobel.
        Assert.Equal(ClipboardHistoryBuffer.MaxItems, buffer.Items.Count);
        Assert.Equal("teks 10", buffer.Items[0].Text);
        Assert.Single(buffer.Items, x => x.Text == "teks 10");
    }

    [Fact]
    public void SpeedFormatSwitchesToMegabytes()
    {
        Assert.Equal("0 KB/s", MainWindow.FormatSpeed(100));
        Assert.Equal("512 KB/s", MainWindow.FormatSpeed(512 * 1024));
        Assert.Equal("1.5 MB/s", MainWindow.FormatSpeed(1.5 * 1024 * 1024));
        Assert.Equal("250 MB/s", MainWindow.FormatSpeed(250.0 * 1024 * 1024));
    }

    [Fact]
    public void AnimationFpsDividesRefreshRate()
    {
        Assert.Equal(100, DesktopNative.PacedFrames(200));
        Assert.Equal(90, DesktopNative.PacedFrames(180));
        Assert.Equal(72, DesktopNative.PacedFrames(144));
        Assert.Equal(120, DesktopNative.PacedFrames(240));
        Assert.Equal(60, DesktopNative.PacedFrames(60));
        Assert.Equal(120, DesktopNative.PacedFrames(120));
    }

    // Jadwal Kemenag 2 November 2026 dari api.myquran.com (Subuh, Terbit, Dzuhur, Ashar, Maghrib, Isya).
    [Theory]
    [InlineData("Jakarta", "04:08 05:23 11:40 14:55 17:49 19:01")]
    [InlineData("Surabaya", "03:43 04:58 11:16 14:31 17:27 18:39")]
    [InlineData("Makassar", "04:19 05:34 11:49 15:06 17:58 19:09")]
    public void PrayerTimesMatchKemenagWithinOneMinute(string city, string official)
    {
        var computed = PrayerTimes.For(new DateOnly(2026, 11, 2), PrayerCities.Find(city));
        var expected = official.Split(' ').Select(TimeSpan.Parse).ToArray();
        for (var i = 0; i < 6; i++)
            Assert.True(Math.Abs((computed[i].Time.TimeOfDay - expected[i]).TotalMinutes) <= 1,
                $"{city} {computed[i].Name}: {computed[i].Time:HH:mm}, resmi {expected[i]:hh\\:mm}");
    }
    [Fact]
    public void PageNavigationSkipsDisabledPages()
    {
        List<int> pages = [0, 2, 3];
        Assert.Equal(2, NotchPages.Next(pages, 0, 1));
        Assert.Equal(0, NotchPages.Next(pages, 3, 1));
        Assert.Equal(3, NotchPages.Next(pages, 0, -1));
        Assert.Equal(0, NotchPages.Next(pages, 1, 1)); // Halaman aktif sudah dimatikan: kembali ke Musik.
        Assert.Equal(0, NotchPages.Next([0], 0, 1));
    }

    [Fact]
    public void CountdownReadsNaturally()
    {
        Assert.Equal("59 menit lagi", MainWindow.Countdown(TimeSpan.FromMinutes(58.2)));
        Assert.Equal("1 jam lagi", MainWindow.Countdown(TimeSpan.FromMinutes(59.5)));
        Assert.Equal("1 jam 5 menit lagi", MainWindow.Countdown(TimeSpan.FromMinutes(65)));
        Assert.Equal("kurang dari 1 menit", MainWindow.Countdown(TimeSpan.Zero));
    }
}