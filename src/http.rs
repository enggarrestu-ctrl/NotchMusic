// HTTPS GET lewat WinHTTP: memakai sertifikat dan proxy Windows, jadi tetap jalan di jaringan kantor
// yang memeriksa TLS. Respons dibatasi ukurannya agar memori tetap kecil.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::*;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const TIMEOUT_MS: i32 = 15_000;

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
}

/// Handle WinHTTP yang ditutup otomatis.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `url` harus https://host/path?query.
pub fn get(url: &str) -> Result<Response, String> {
    send("GET", url, None)
}

/// POST dengan badan JSON (Unison /translate).
pub fn post_json(url: &str, body: &str) -> Result<Response, String> {
    send("POST", url, Some(body.as_bytes()))
}

fn send(verb: &str, url: &str, body: Option<&[u8]>) -> Result<Response, String> {
    let rest = url.strip_prefix("https://").ok_or("hanya https")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host_w, path_w) = (wide(host), wide(path));
    let agent = wide(&format!("NotchRust/{} (personal Windows desktop project)", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session = Handle(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err("WinHTTP tidak tersedia".into());
        }
        let _ = WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);
        let connection = Handle(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if connection.0.is_null() {
            return Err("tidak bisa terhubung".into());
        }
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            PCWSTR(wide(verb).as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err("permintaan gagal dibuat".into());
        }
        let headers = if body.is_some() { "Accept: application/json\r\nContent-Type: application/json\r\n" } else { "Accept: application/json\r\n" };
        let headers: Vec<u16> = headers.encode_utf16().collect();
        let (data, length) = body.map_or((None, 0), |b| (Some(b.as_ptr() as *const c_void), b.len() as u32));
        WinHttpSendRequest(request.0, Some(&headers), data, length, length, 0).map_err(|e| e.message())?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| e.message())?;

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            &mut index,
        )
        .map_err(|e| e.message())?;

        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read)
                .map_err(|e| e.message())?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > MAX_BYTES {
                return Err("respons terlalu besar".into());
            }
        }
        Ok(Response { status, body })
    }
}

/// Persen-encode untuk parameter query (UTF-8).
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_escapes_spaces_and_unicode() {
        assert_eq!(super::encode("a b&c"), "a%20b%26c");
        assert_eq!(super::encode("é"), "%C3%A9");
    }
}
