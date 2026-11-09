// Kecepatan internet: selisih total byte semua adapter fisik yang aktif per detik.
// Dikerjakan di thread sendiri agar membaca tabel adapter tidak membuat animasi tersendat.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Speed {
    pub down: f64,
    pub up: f64,
}

pub fn spawn(hwnd: isize, msg: u32, shared: Arc<Mutex<Speed>>) {
    thread::spawn(move || {
        let mut last: Option<(u64, u64, Instant)> = None;
        loop {
            if let Some((down, up)) = totals() {
                let now = Instant::now();
                if let Some((d0, u0, t0)) = last {
                    let seconds = now.duration_since(t0).as_secs_f64();
                    if seconds > 0.2 {
                        // Adapter putus/tersambung bisa membuat total turun: anggap 0, bukan negatif.
                        let speed = Speed {
                            down: down.saturating_sub(d0) as f64 / seconds,
                            up: up.saturating_sub(u0) as f64 / seconds,
                        };
                        *shared.lock().unwrap() = speed;
                        unsafe {
                            let _ = PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                last = Some((down, up, now));
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn totals() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let (mut down, mut up) = (0u64, 0u64);
        for row in rows {
            // Bit 0 = HardwareInterface: lewati adapter virtual/filter agar byte tidak terhitung dobel.
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.OperStatus == IfOperStatusUp && row.Type != IF_TYPE_SOFTWARE_LOOPBACK {
                down += row.InOctets;
                up += row.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        Some((down, up))
    }
}

pub fn format_speed(bytes_per_second: f64) -> String {
    let kb = bytes_per_second / 1024.0;
    if kb < 1000.0 {
        return format!("{:.0} KB/s", kb.round());
    }
    let mb = kb / 1024.0;
    if mb < 100.0 { format!("{mb:.1} MB/s") } else { format!("{mb:.0} MB/s") }
}

#[cfg(test)]
mod tests {
    use super::format_speed;

    #[test]
    fn speed_format_switches_to_megabytes() {
        assert_eq!(format_speed(100.0), "0 KB/s");
        assert_eq!(format_speed(512.0 * 1024.0), "512 KB/s");
        assert_eq!(format_speed(1.5 * 1024.0 * 1024.0), "1.5 MB/s");
        assert_eq!(format_speed(250.0 * 1024.0 * 1024.0), "250 MB/s");
    }
}
