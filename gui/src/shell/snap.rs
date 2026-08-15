//! The control channel's `snap` — any HWND's client area, as a PNG.
//!
//! **WINARM (WG1): this file is SHARED, not copied.** It is the capture that
//! `macvm-gui` has had since P4, lifted out of `shell/win.rs` unchanged and
//! given its HWND as a parameter instead of reading the module's own. `win.rs`
//! keeps a one-line `snap_client_area` that supplies its window; `win_gui`
//! (`macvm-winui`) includes this exact source with `#[path]` and supplies its
//! own. `win_gui_design.md` §3.1's point is that the capture channel already
//! exists and WG1 inherits it — a second implementation of it would be a
//! regression dressed as a deliverable, and two copies of a PNG writer drift
//! in ways nobody notices until a shot is unreadable.
//!
//! **Why `PrintWindow` and not WebView2's `CapturePreview`.** `CapturePreview`
//! is the "official" WebView2 answer, but it is ASYNCHRONOUS, and `win.rs`'s
//! header records what that costs on this thread: an async WebView2 call
//! awaited from the UI thread runs a nested message loop, which re-enters and
//! can deadlock (the P4 finding that made `eval_js` fire-and-forget).
//! `PrintWindow` with `PW_RENDERFULLCONTENT` is synchronous, needs no
//! completion handler, and exists precisely to capture windows whose content
//! is composed off the legacy GDI path — which is what a WebView2 child is.
//!
//! That choice is also exactly what makes this reusable: it captures *any*
//! HWND's client area with no knowledge of what drew it, so the native
//! Smalltalk window works through it unchanged, on the first try, with no
//! WebView2 in the process at all.
//!
//! The UI thread does the whole capture inline and answers immediately;
//! nothing blocks but the LISTENER thread, which is a plain worker with
//! nothing else to do. Blocking the right thread is the whole trick.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

/// Capture `h`'s client area to `path` as a PNG and answer `reply`.
///
/// `reply` is answered exactly once on every path, including every failure —
/// a dropped channel would surface to the script only as the listener's
/// 20-second timeout, which reads like a hang rather than the error it is.
/// A null HWND is the ordinary "the window is not up yet" case and says so,
/// which is a gate item in its own right: `snap` before there is a window must
/// be a named error, not a hang and not a zero-byte file.
pub fn snap_hwnd(h: HWND, path: &str, reply: std::sync::mpsc::SyncSender<String>) {
    if h.0.is_null() {
        let _ = reply.send("ERR no window yet".into());
        return;
    }
    match capture_png(h, path) {
        Ok(()) => {
            let _ = reply.send("OK".into());
        }
        Err(e) => {
            let _ = reply.send(format!("ERR {e}"));
        }
    }
}

/// Capture `h` to `path` as a PNG.
///
/// **The bitmap is sized by `GetWindowRect`, not `GetClientRect`** — and the
/// difference was a real defect, found by looking at a shot rather than at
/// its dimensions. `PrintWindow` renders the WHOLE window (frame, titlebar,
/// client) into the target DC starting at (0,0). Sizing the bitmap to the
/// client rect therefore produced an image that *measured* correct — client
/// 900×600, PNG 900×600, the gate's equality satisfied — while actually
/// containing the titlebar plus only the top 560 px of the client area, the
/// rest silently cropped. On WG1's empty window that is invisible; the first
/// time WG3 draws a control near the bottom it would be a mystery.
///
/// Capturing the window rect also matches the reference gallery
/// (`MACVM/docs/gallery/`), whose every shot includes the frame — which is
/// what makes two platforms' screenshots comparable at a glance.
pub fn capture_png(h: HWND, path: &str) -> std::result::Result<(), String> {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(h, &mut rect).map_err(|e| format!("GetWindowRect: {e}"))?;
        let (w, ht) = (rect.right - rect.left, rect.bottom - rect.top);
        if w <= 0 || ht <= 0 {
            return Err("window rect is empty".into());
        }

        let screen: HDC = GetDC(Some(h));
        if screen.is_invalid() {
            return Err("GetDC failed".into());
        }
        let mem: HDC = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            ReleaseDC(Some(h), screen);
            return Err("CreateCompatibleDC failed".into());
        }

        // Top-down 32bpp so the row order matches PNG's and no flip is needed.
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -ht;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib: HBITMAP =
            match CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !b.is_invalid() && !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(mem);
                    ReleaseDC(Some(h), screen);
                    return Err("CreateDIBSection failed".into());
                }
            };
        let old: HGDIOBJ = SelectObject(mem, dib.into());

        // PW_RENDERFULLCONTENT (0x2) is the flag that makes this work at all
        // for a WebView2 child: without it the composed content is missing and
        // the shot comes back blank/white.
        let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(2u32)).as_bool();

        let result = if ok {
            let n = (w as usize) * (ht as usize) * 4;
            let src = std::slice::from_raw_parts(bits as *const u8, n);
            // BGRA (GDI) -> RGBA (PNG), forcing alpha opaque: PrintWindow
            // leaves the alpha channel undefined, and honouring it yields a
            // fully transparent image that looks like a capture failure.
            let mut rgba = Vec::with_capacity(n);
            for px in src.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
            }
            write_png(path, w as u32, ht as u32, &rgba)
        } else {
            Err("PrintWindow failed".into())
        };

        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(Some(h), screen);
        result
    }
}

/// Write an RGBA buffer as a PNG, by hand.
///
/// No image crate: this GUI's dependency list is deliberately short, and a
/// debugging snapshot does not justify a new one. PNG's uncompressed-deflate
/// (BTYPE=00) stored blocks make a valid file writable in ~40 lines — larger
/// than a compressed one, which costs nothing for a screenshot read once.
pub fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> std::result::Result<(), String> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, e) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for &b in bytes {
            c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        c ^ 0xFFFF_FFFF
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_in = Vec::with_capacity(4 + data.len());
        crc_in.extend_from_slice(kind);
        crc_in.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
    }

    // Scanlines with PNG filter byte 0 (None) in front of each row.
    let mut raw = Vec::with_capacity((h as usize) * (1 + (w as usize) * 4));
    for y in 0..h as usize {
        raw.push(0u8);
        let off = y * (w as usize) * 4;
        raw.extend_from_slice(&rgba[off..off + (w as usize) * 4]);
    }

    // zlib stream: 0x78 0x01 header, stored deflate blocks, adler32 trailer.
    let mut z = vec![0x78u8, 0x01];
    for (i, part) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(part.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
        z.extend_from_slice(part);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter/interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, &png).map_err(|e| format!("write {path}: {e}"))
}
