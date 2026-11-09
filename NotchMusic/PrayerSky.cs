using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;

namespace NotchMusic;

// Halaman salat: matahari bergerak di busur Terbit -> Dzuhur (puncak) -> Maghrib; Subuh dan Isya di cakrawala.
public partial class MainWindow
{
    private void DrawPrayerSky(List<PrayerTime> times, DateTime now, PrayerTime? next)
    {
        var sky = PrayerSky;
        sky.Children.Clear();
        var layout = new SkyLayout(times, sky.Width, sky.Height);
        var accent = (Brush)FindResource("NotchAccent");
        var dim = new SolidColorBrush(Color.FromArgb(0x55, 0xFF, 0xFF, 0xFF)); dim.Freeze();
        var sunBrush = new SolidColorBrush(Color.FromRgb(0xFF, 0xD5, 0x6B)); sunBrush.Freeze();

        // Cakrawala, busur siang (tipis putus-putus), lalu bagian yang sudah dilewati matahari (warna aksen).
        sky.Children.Add(new Line { X1 = 0, X2 = sky.Width, Y1 = layout.Horizon, Y2 = layout.Horizon, Stroke = dim, StrokeThickness = 1 });
        sky.Children.Add(new Path { Data = layout.Arc(layout.Sunrise, layout.Sunset), Stroke = dim, StrokeThickness = 1.5, StrokeDashArray = [2, 3] });
        if (now > layout.Sunrise)
            sky.Children.Add(new Path { Data = layout.Arc(layout.Sunrise, now < layout.Sunset ? now : layout.Sunset),
                Stroke = accent, StrokeThickness = 2, StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round });

        foreach (var time in times)
        {
            var (x, y) = layout.Point(time.Time);
            var isNext = time == next;
            var past = time.Time <= now;
            var marker = new Ellipse { Width = isNext ? 9 : 6, Height = isNext ? 9 : 6, Fill = past ? accent : Brushes.Black,
                Stroke = isNext || past ? accent : Brushes.White, StrokeThickness = isNext ? 2 : 1.2, Opacity = past && !isNext ? 0.6 : 1 };
            Place(marker, x - marker.Width / 2, y - marker.Height / 2);
            // Terbit dan Maghrib: matahari setengah di cakrawala (terbit/terbenam), titik biasa hanya bila jadi berikutnya.
            if (time == times[1] || time == times[4])
            {
                // Penanda tetap (bukan status), jadi selalu warna penuh: setengah lingkaran + tiga sinar.
                sky.Children.Add(new Path { Fill = sunBrush, Stroke = sunBrush, StrokeThickness = 1.4, IsHitTestVisible = false,
                    StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round,
                    Data = Geometry.Parse(FormattableString.Invariant(
                        $"M{x - 7},{y} A7,7 0 0 1 {x + 7},{y} Z M{x},{y - 9.5} L{x},{y - 12.5} M{x - 7.4},{y - 6.8} L{x - 9.5},{y - 8.9} M{x + 7.4},{y - 6.8} L{x + 9.5},{y - 8.9}")) });
                if (!isNext) marker.Visibility = Visibility.Collapsed;
            }
            // Semua label sejajar di bawah cakrawala seperti sumbu waktu (posisi konsisten, UI/UX Pro Max);
            // titik di busur (Dzuhur, Ashar) disambung ke labelnya dengan garis tipis.
            if (y < layout.Horizon - 1)
                sky.Children.Add(new Line { X1 = x, X2 = x, Y1 = y + 5, Y2 = layout.Horizon, Stroke = dim, StrokeThickness = 1,
                    StrokeDashArray = [1.5, 2.5] });
            y = layout.Horizon;
            var label = new StackPanel { Width = 52, Opacity = past && !isNext ? 0.5 : 1 };
            label.Children.Add(new TextBlock { Text = time.Name, FontSize = 9, Foreground = isNext ? accent : Brushes.White,
                Opacity = isNext ? 1 : 0.8, TextAlignment = TextAlignment.Center });
            label.Children.Add(new TextBlock { Text = time.Time.ToString("HH.mm", CultureInfo.InvariantCulture), FontSize = 12,
                FontWeight = FontWeights.SemiBold, Foreground = isNext ? accent : Brushes.White, TextAlignment = TextAlignment.Center });
            Place(label, x - 26, y + 5);
            sky.Children.Add(marker);
            sky.Children.Add(label);
        }

        // Siang: matahari bercahaya di busur. Malam: bulan sabit di cakrawala.
        var (sx, sy) = layout.Point(now);
        if (now > layout.Sunrise && now < layout.Sunset)
        {
            var glow = new Ellipse { Width = 22, Height = 22, IsHitTestVisible = false,
                Fill = new RadialGradientBrush(Color.FromArgb(0x90, 0xFF, 0xD5, 0x6B), Color.FromArgb(0, 0xFF, 0xD5, 0x6B)) };
            var sun = new Ellipse { Width = 11, Height = 11, Fill = sunBrush, IsHitTestVisible = false };
            sky.Children.Add(Place(glow, sx - 11, sy - 11));
            sky.Children.Add(Place(sun, sx - 5.5, sy - 5.5));
        }

        AutomationProperties.SetHelpText(sky, string.Join(", ", times.Select(x => $"{x.Name} {x.Time:HH.mm}")));
    }

    private static T Place<T>(T element, double x, double y) where T : UIElement
    {
        Canvas.SetLeft(element, x); Canvas.SetTop(element, y);
        return element;
    }
}

// Posisi murni (tanpa UI) agar bisa dites: waktu -> titik di kanvas.
internal sealed class SkyLayout(List<PrayerTime> times, double width, double height)
{
    private const double Edge = 26, Dawn = 76; // Jarak Subuh/Isya dan Terbit/Maghrib dari tepi kanvas.
    public double Horizon => height - 32;
    private double Peak => Horizon - 8; // Tinggi busur: Dzuhur tetap muat bersama labelnya di bawah titik.
    private DateTime Subuh => times[0].Time;
    public DateTime Sunrise => times[1].Time;
    public DateTime Sunset => times[4].Time;
    private DateTime Isya => times[5].Time;

    public double X(DateTime t)
    {
        static double Lerp(double a, double b, DateTime from, DateTime to, DateTime t)
            => a + (b - a) * Math.Clamp((t - from).TotalMinutes / Math.Max(1, (to - from).TotalMinutes), 0, 1);
        if (t <= Subuh) return Math.Max(6, Edge - (Subuh - t).TotalMinutes / 30);
        if (t <= Sunrise) return Lerp(Edge, Dawn, Subuh, Sunrise, t);
        if (t <= Sunset) return Lerp(Dawn, width - Dawn, Sunrise, Sunset, t);
        if (t <= Isya) return Lerp(width - Dawn, width - Edge, Sunset, Isya, t);
        return Math.Min(width - 6, width - Edge + (t - Isya).TotalMinutes / 30);
    }

    public double Y(DateTime t)
    {
        if (t <= Sunrise || t >= Sunset) return Horizon;
        var f = (t - Sunrise).TotalMinutes / (Sunset - Sunrise).TotalMinutes;
        return Horizon - Peak * Math.Sin(Math.PI * f);
    }

    public (double X, double Y) Point(DateTime t) => (X(t), Y(t));

    public Geometry Arc(DateTime from, DateTime to)
    {
        var figure = new PathFigure { StartPoint = new Point(X(from), Y(from)) };
        const int steps = 48;
        for (var i = 1; i <= steps; i++)
        {
            var t = from + (to - from) * (i / (double)steps);
            figure.Segments.Add(new LineSegment(new Point(X(t), Y(t)), true));
        }
        var geometry = new PathGeometry([figure]);
        geometry.Freeze();
        return geometry;
    }
}
