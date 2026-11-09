// Deteksi game/aplikasi layar penuh di monitor notch (port DesktopNative.FullscreenAppOnMonitor NotchMusic).

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    IsZoomed, GWL_STYLE, WS_CAPTION,
};

/// Jendela depan milik aplikasi lain menutupi seluruh monitor notch: game atau aplikasi layar penuh.
/// Jendela maximize ber-title bar tidak dihitung (dengan taskbar auto-hide ia juga menutupi monitor);
/// game borderless sering juga ber-status maximize tetapi tanpa title bar.
pub fn fullscreen_app_on_monitor(notch: HWND) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if notch.is_invalid() || foreground.is_invalid() || foreground == GetShellWindow() {
            return false;
        }
        let caption = WS_CAPTION.0 as isize;
        if IsZoomed(foreground).as_bool() && GetWindowLongPtrW(foreground, GWL_STYLE) & caption == caption {
            return false;
        }
        let mut process = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut process));
        if process == GetCurrentProcessId() {
            return false;
        }
        let mut class = [0u16; 16];
        let length = GetClassNameW(foreground, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..length]);
        if class == "WorkerW" || class == "Progman" {
            return false; // Desktop.
        }
        let monitor = MonitorFromWindow(notch, MONITOR_DEFAULTTONEAREST);
        if MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) != monitor {
            return false;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut window = RECT::default();
        GetMonitorInfoW(monitor, &mut info).as_bool() && GetWindowRect(foreground, &mut window).is_ok() && covers(&window, &info.rcMonitor)
    }
}

/// Jendela menutupi seluruh monitor (boleh lebih besar, seperti game exclusive/borderless).
pub fn covers(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left && window.top <= monitor.top && window.right >= monitor.right && window.bottom >= monitor.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_only_full_monitor() {
        let monitor = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert!(covers(&RECT { left: -8, top: -8, right: 1928, bottom: 1088 }, &monitor));
        assert!(covers(&monitor, &monitor));
        assert!(!covers(&RECT { left: 0, top: 0, right: 1920, bottom: 1040 }, &monitor)); // Di atas taskbar.
    }
}
