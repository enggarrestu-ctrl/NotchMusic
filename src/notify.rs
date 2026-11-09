// Notifikasi WhatsApp/browser dan controller tersambung/terputus, ditampilkan sebentar di notch kecil
// (port StartNotificationsAsync / PollNotificationsAsync / StartPadWatch NotchMusic).
// Windows tidak mengirim event notifikasi ke aplikasi tanpa paket (MSIX), jadi daftarnya dibaca tiap 2 detik.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{IInspectable, Result};
use windows::Foundation::EventHandler;
use windows::Gaming::Input::RawGameController;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_ACCEPTS_NOTIFICATIONS};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// WhatsApp Web di browser: Edge/Chrome tidak menyebut situs asal, jadi semua notifikasi browser ditampilkan.
const SHOWN_APPS: [&str; 6] = ["WhatsApp", "Microsoft Edge", "Google Chrome", "Brave", "Opera", "Firefox"];

pub struct Shared {
    /// Teks notifikasi terbaru yang belum diambil UI.
    pub pending: Mutex<Option<String>>,
    /// Fitur "Notifikasi dan kecepatan internet" aktif.
    pub enabled: AtomicBool,
}

pub fn spawn(hwnd: isize, message: u32, enabled: bool) -> Arc<Shared> {
    let shared = Arc::new(Shared { pending: Mutex::new(None), enabled: AtomicBool::new(enabled) });
    let poll = shared.clone();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if !allowed() {
            return; // Izin "akses notifikasi" dimatikan di Windows: fitur diam saja.
        }
        let mut seen: Option<HashSet<u32>> = None; // None = daftar awal belum dibaca; notifikasi lama tidak muncul.
        loop {
            if poll.enabled.load(Ordering::Relaxed) {
                if let Ok(shown) = toasts() {
                    let fresh = seen
                        .as_ref()
                        .and_then(|seen| shown.iter().filter(|t| !seen.contains(&t.0)).max_by_key(|t| t.1))
                        .map(|t| t.2.clone());
                    seen = Some(shown.iter().map(|t| t.0).collect()); // Mengikuti isi Action Center, tidak tumbuh terus.
                    // Saat presentasi/berbagi layar/aplikasi layar penuh, isi pesan tidak ditampilkan.
                    if let Some(text) = fresh.filter(|_| accepts_notifications()) {
                        post(&poll, hwnd, message, text);
                    }
                }
            } else {
                seen = None; // Saat dinyalakan lagi, notifikasi lama tidak muncul.
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    shared
}

/// Controller tersambung/terputus. Controller yang sudah tersambung dilaporkan ulang saat aplikasi mulai: diabaikan 3 detik.
pub fn watch_pads(shared: &Arc<Shared>, hwnd: isize, message: u32) {
    let started = Instant::now();
    let handler = |text: &'static str| {
        let shared = shared.clone();
        EventHandler::<RawGameController>::new(move |_: Option<&IInspectable>, _: &Option<RawGameController>| {
            if started.elapsed() > Duration::from_secs(3) && shared.enabled.load(Ordering::Relaxed) {
                post(&shared, hwnd, message, text.to_string());
            }
            Ok(())
        })
    };
    let _ = RawGameController::RawGameControllerAdded(&handler("Controller terhubung"));
    let _ = RawGameController::RawGameControllerRemoved(&handler("Controller terputus"));
    let _ = RawGameController::RawGameControllers(); // Akses pertama menyalakan pemantauan.
}

fn post(shared: &Shared, hwnd: isize, message: u32, text: String) {
    *shared.pending.lock().unwrap() = Some(text);
    unsafe {
        let _ = PostMessageW(HWND(hwnd as _), message, WPARAM(0), LPARAM(0));
    }
}

fn allowed() -> bool {
    let Ok(listener) = UserNotificationListener::Current() else { return false };
    if listener.GetAccessStatus().is_ok_and(|s| s == UserNotificationListenerAccessStatus::Allowed) {
        return true;
    }
    listener
        .RequestAccessAsync()
        .and_then(|request| request.get())
        .is_ok_and(|s| s == UserNotificationListenerAccessStatus::Allowed)
}

/// Windows sendiri menahan banner saat layar terkunci, presentasi, atau aplikasi/game layar penuh.
fn accepts_notifications() -> bool {
    unsafe { SHQueryUserNotificationState().map_or(true, |state| state == QUNS_ACCEPTS_NOTIFICATIONS) }
}

/// Notifikasi dari aplikasi yang ditampilkan: (id, waktu dibuat, teks ringkas).
fn toasts() -> Result<Vec<(u32, i64, String)>> {
    let list = UserNotificationListener::Current()?.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    let mut shown = Vec::new();
    for toast in list {
        if !is_shown(&toast) {
            continue;
        }
        shown.push((toast.Id()?, toast.CreationTime()?.UniversalTime, text(&toast)));
    }
    Ok(shown)
}

fn is_shown(toast: &UserNotification) -> bool {
    // Beberapa aplikasi menolak membuka info tampilannya: anggap bukan WhatsApp/browser.
    let name = toast.AppInfo().and_then(|info| info.DisplayInfo()).and_then(|info| info.DisplayName()).map(|n| n.to_string());
    name.is_ok_and(|name| SHOWN_APPS.iter().any(|app| name.to_lowercase().contains(&app.to_lowercase())))
}

fn text(toast: &UserNotification) -> String {
    let texts: Vec<String> = (|| -> Result<Vec<String>> {
        let binding = toast.Notification()?.Visual()?.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
        Ok(binding.GetTextElements()?.into_iter().filter_map(|t| t.Text().ok()).map(|t| t.to_string().trim().to_string()).filter(|t| !t.is_empty()).collect())
    })()
    .unwrap_or_default();
    summarize(&texts)
}

/// "Pengirim: isi pesan", spasi dirapikan, maksimal 80 karakter.
pub fn summarize(texts: &[String]) -> String {
    let Some(first) = texts.first() else { return "Notifikasi baru".into() };
    let line = if texts.len() > 1 { format!("{first}: {}", texts[1..].join(" ")) } else { first.clone() };
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 80 {
        line.chars().take(79).collect::<String>() + "…"
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::summarize;

    #[test]
    fn summary_joins_sender_and_message() {
        assert_eq!(summarize(&[]), "Notifikasi baru");
        assert_eq!(summarize(&["Ibu".into(), "Sudah  makan?\nNanti\tpulang".into()]), "Ibu: Sudah makan? Nanti pulang");
        let long = "x".repeat(100);
        assert_eq!(summarize(&[long]).chars().count(), 80);
    }
}
