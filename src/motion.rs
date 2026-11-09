// Gerak notch (port Motion.cs NotchMusic; panduan UI/UX Pro Max: spring-physics,
// exit-faster-than-enter, stagger-sequence).

/// Membuka notch.
pub const OPEN_SECONDS: f32 = 0.34;
/// Menutup lebih singkat dari membuka.
pub const CLOSE_SECONDS: f32 = 0.20;
/// Pindah halaman (shared axis: geser pendek + ganti pudar).
pub const PAGE_SECONDS: f32 = 0.28;
/// Pantulan saat membuka (redaman < 1 memantul ~4%).
pub const OPEN_DAMPING: f32 = 0.72;

/// Pegas teredam ternormalisasi: f(0) = 0, f(1) = 1. damping < 1 memantul sedikit; 1 = teredam kritis.
pub fn spring(p: f32, damping: f32) -> f32 {
    let raw = |t: f32| -> f32 {
        let zeta = damping.clamp(0.3, 1.0);
        if zeta >= 0.999 {
            let omega = 9.0; // Selubung e^-9 di akhir: praktis sudah diam.
            return 1.0 - (-omega * t).exp() * (1.0 + omega * t);
        }
        let omega0 = 7.0 / zeta; // zeta*omega = 7: getaran habis tepat di akhir durasi.
        let omega_d = omega0 * (1.0 - zeta * zeta).sqrt();
        1.0 - (-zeta * omega0 * t).exp() * ((omega_d * t).cos() + zeta * omega0 / omega_d * (omega_d * t).sin())
    };
    raw(p.clamp(0.0, 1.0)) / raw(1.0)
}

/// Halaman shared axis: (geser keluar, alpha keluar, geser masuk, alpha masuk) untuk progres 0..1.
/// Halaman lama hilang di sepertiga awal; halaman baru baru muncul sesudahnya, jadi teks tidak menumpuk.
pub fn shared_axis(p: f32, direction: f32, distance: f32) -> (f32, f32, f32, f32) {
    let eased = spring(p, 1.0);
    let swap = 0.35;
    let out_alpha = 1.0 - (p / swap).min(1.0);
    let in_alpha = ((p - swap) / (1.0 - swap)).clamp(0.0, 1.0);
    (-direction * distance * eased, out_alpha, direction * distance * (1.0 - eased), spring(in_alpha, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_lands_exactly_with_small_bounce() {
        assert!(spring(0.0, OPEN_DAMPING).abs() < 1e-6 && (spring(1.0, OPEN_DAMPING) - 1.0).abs() < 1e-6);
        let peak = (0..=200).map(|i| spring(i as f32 / 200.0, OPEN_DAMPING)).fold(0.0, f32::max);
        assert!((1.02..=1.07).contains(&peak), "pantulan {peak}");
        assert!((0..=200).all(|i| spring(i as f32 / 200.0, 1.0) <= 1.0001)); // Menutup tanpa pantulan.
        assert!(spring(0.3, OPEN_DAMPING) > 0.7); // Cepat di awal: terasa responsif.
    }

    #[test]
    fn shared_axis_never_overlaps_old_and_new_text() {
        for i in 0..=100 {
            let (_, out_alpha, _, in_alpha) = shared_axis(i as f32 / 100.0, 1.0, 48.0);
            assert!(out_alpha == 0.0 || in_alpha == 0.0);
        }
        assert_eq!(shared_axis(1.0, 1.0, 48.0).2.abs() < 0.01, true);
    }
}
