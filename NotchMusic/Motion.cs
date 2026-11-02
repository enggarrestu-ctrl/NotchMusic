using System.Windows;
using System.Windows.Media.Animation;

namespace NotchMusic;

// Gerak notch (panduan UI/UX Pro Max: spring-physics, exit-faster-than-enter, stagger-sequence).
internal static class Motion
{
    public const int OpenMilliseconds = 340, CloseMilliseconds = 200, ContentDelayMilliseconds = 90;

    // Membuka: pegas dengan pantulan kecil (~4%), seperti Dynamic Island.
    public static readonly IEasingFunction Open = Frozen(new SpringEase { Damping = 0.72 });
    // Menutup/mengecil: pegas tanpa pantulan, cepat lalu mendarat halus.
    public static readonly IEasingFunction Settle = Frozen(new SpringEase { Damping = 1 });

    private static SpringEase Frozen(SpringEase ease) { ease.Freeze(); return ease; }
}

// Pegas teredam ternormalisasi: f(0) = 0, f(1) = 1. Damping < 1 memantul sedikit; 1 = teredam kritis (tanpa pantulan).
internal sealed class SpringEase : EasingFunctionBase
{
    public static readonly DependencyProperty DampingProperty =
        DependencyProperty.Register(nameof(Damping), typeof(double), typeof(SpringEase), new PropertyMetadata(0.72));

    public double Damping { get => (double)GetValue(DampingProperty); set => SetValue(DampingProperty, value); }

    public SpringEase() => EasingMode = EasingMode.EaseIn; // EaseIn = pakai kurva di bawah apa adanya.

    protected override double EaseInCore(double t) => Raw(t) / Raw(1);

    private double Raw(double t)
    {
        var zeta = Math.Clamp(Damping, 0.3, 1);
        if (zeta >= 0.999)
        {
            const double omega = 9; // Selubung e^-9 di akhir: praktis sudah diam.
            return 1 - Math.Exp(-omega * t) * (1 + omega * t);
        }
        var omega0 = 7 / zeta; // zeta*omega = 7: getaran habis tepat di akhir durasi.
        var omegaD = omega0 * Math.Sqrt(1 - zeta * zeta);
        return 1 - Math.Exp(-zeta * omega0 * t) * (Math.Cos(omegaD * t) + zeta * omega0 / omegaD * Math.Sin(omegaD * t));
    }

    protected override Freezable CreateInstanceCore() => new SpringEase();
}
