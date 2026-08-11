//! The device, the swapchain, and the seam the guest calls.
//!
//! Everything in `lib.rs` draws into any `ID2D1RenderTarget` and is proven
//! against a WIC bitmap with no window. This module is the plumbing that
//! points it at a real pane, plus the `#[no_mangle]` entry points the guest
//! reaches through the FFI's `library:` part — the exact `winui_host` shape,
//! so the VM learns nothing new.

use std::cell::RefCell;
use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, ID2D1RenderTarget,
    ID2D1SolidColorBrush, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::IDWriteFontFace;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM as FMT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::{client_size, colorref_to_d2d, draw_grid, mono_face_metrics, rt_props, Cell, CellMetrics};

struct Renderer {
    hwnd: HWND,
    face: IDWriteFontFace,
    metrics: CellMetrics,
    ctx: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    brush: ID2D1SolidColorBrush,
    target: Option<ID2D1Bitmap1>,
    px_w: u32,
    px_h: u32,
    cols: u32,
    rows: u32,
    cells: Vec<Cell>,
    caret: Option<(u32, u32)>,
    frames: u64,
}

thread_local! {
    /// ONE renderer, on the UI thread, and `thread_local` rather than a
    /// `static` because COM interfaces are not `Send`. The pane, the pump and
    /// every VM entry are all on that thread — the same quiescence the door
    /// already relies on — so no lock is needed and none would help.
    static R: RefCell<Option<Renderer>> = const { RefCell::new(None) };
    static LAST_ERROR: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

fn set_error(s: &str) {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    LAST_ERROR.with(|e| *e.borrow_mut() = v);
}

/// The `winui_host` status convention, verbatim — 0 ok, non-zero with the
/// reason in the message slot — so the guest reads this channel exactly the
/// way it already reads that one.
const OK: i64 = 0;
const ERR: i64 = 1;

fn finish(r: Result<(), String>) -> i64 {
    match r {
        Ok(()) => {
            set_error("");
            OK
        }
        Err(e) => {
            set_error(&e);
            ERR
        }
    }
}

fn build(hwnd: HWND, pt: f32, dpi: f32) -> Result<Renderer, String> {
    unsafe {
        let (face, metrics) = mono_face_metrics(pt, dpi)?;
        let (px_w, px_h) = client_size(hwnd)?;

        // HARDWARE, falling back to WARP. A machine with no usable GPU still
        // gets a pane; it renders on the CPU, which for a few thousand glyphs
        // is entirely adequate and is why the fallback is silent.
        let mut device: Option<ID3D11Device> = None;
        let mut hr = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        );
        if hr.is_err() {
            hr = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            );
        }
        hr.map_err(|e| format!("D3D11CreateDevice: {e}"))?;
        let device = device.ok_or("D3D11CreateDevice answered no device")?;
        let dxgi_dev: IDXGIDevice = device.cast().map_err(|e| format!("cast IDXGIDevice: {e}"))?;

        let dxgi_factory: IDXGIFactory2 =
            CreateDXGIFactory2(Default::default()).map_err(|e| format!("CreateDXGIFactory2: {e}"))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: px_w,
            Height: px_h,
            Format: FMT,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            // FLIP_SEQUENTIAL with two buffers: the flip model, which is what
            // makes this tear-free and is the whole reason for a swapchain
            // rather than the legacy hwnd render target.
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: Default::default(),
            Flags: 0,
        };
        let swap = dxgi_factory
            .CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)
            .map_err(|e| format!("CreateSwapChainForHwnd: {e}"))?;

        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory: {e}"))?;
        let d2d_dev = d2d
            .CreateDevice(&dxgi_dev)
            .map_err(|e| format!("ID2D1Factory1::CreateDevice: {e}"))?;
        let ctx = d2d_dev
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(|e| format!("CreateDeviceContext: {e}"))?;
        // DPI PINNED TO 96 so a DIP is a pixel — see `rt_props`. The cell
        // metrics are already physical pixels; a second scale here would be
        // the two-authorities mistake again, somewhere much harder to see.
        ctx.SetDpi(96.0, 96.0);
        let brush = ctx
            .CreateSolidColorBrush(&colorref_to_d2d(0), None)
            .map_err(|e| format!("CreateSolidColorBrush: {e}"))?;

        let mut r = Renderer {
            hwnd,
            face,
            metrics,
            ctx,
            swap,
            brush,
            target: None,
            px_w,
            px_h,
            cols: 0,
            rows: 0,
            cells: Vec::new(),
            caret: None,
            frames: 0,
        };
        r.bind_backbuffer()?;
        Ok(r)
    }
}

impl Renderer {
    /// Point the device context at the swapchain's current back buffer.
    fn bind_backbuffer(&mut self) -> Result<(), String> {
        unsafe {
            let surface: IDXGISurface =
                self.swap.GetBuffer(0).map_err(|e| format!("GetBuffer: {e}"))?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: rt_props().pixelFormat,
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bmp = self
                .ctx
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))
                .map_err(|e| format!("CreateBitmapFromDxgiSurface: {e}"))?;
            self.ctx.SetTarget(&bmp);
            self.target = Some(bmp);
            Ok(())
        }
    }

    /// Follow the pane's client size. `ResizeBuffers` FAILS while the old back
    /// buffer is still referenced, so the target bitmap is released first and
    /// re-created after — the classic, named here so it is not re-discovered.
    fn resize_if_needed(&mut self) -> Result<(), String> {
        let (w, h) = client_size(self.hwnd)?;
        if w == self.px_w && h == self.px_h {
            return Ok(());
        }
        unsafe {
            self.ctx.SetTarget(None);
            self.target = None;
            self.swap
                .ResizeBuffers(0, w, h, FMT, Default::default())
                .map_err(|e| format!("ResizeBuffers: {e}"))?;
        }
        self.px_w = w;
        self.px_h = h;
        self.bind_backbuffer()
    }

    fn present(&mut self) -> Result<(), String> {
        self.resize_if_needed()?;
        let rt: ID2D1RenderTarget = self.ctx.cast().map_err(|e| format!("cast rt: {e}"))?;
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&colorref_to_d2d(0x00FF_FFFF)));
        }
        let drew = draw_grid(
            &rt,
            &self.brush,
            &self.face,
            &self.metrics,
            self.cols,
            self.rows,
            &self.cells,
            self.caret,
        );
        // EndDraw runs on EVERY path, including a failed draw — the same rule
        // `EndPaint` and `CloseClipboard` follow elsewhere in this port.
        let end = unsafe { rt.EndDraw(None, None) };
        drew?;
        end.map_err(|e| format!("EndDraw: {e}"))?;
        unsafe {
            self.swap
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("Present: {e}"))?;
        }
        self.frames += 1;
        Ok(())
    }
}

/// `MacvmRenderAttach(hwnd, ptTenths, dpi)` — device, swapchain, font, all of
/// it, for ONE pane. Re-attaching replaces whatever was there, which is how a
/// DPI change is handled: the shell re-attaches and every derived number is
/// recomputed rather than carried forward stale.
///
/// # Safety
/// `hwnd` must be a live window owned by the calling thread.
#[no_mangle]
pub unsafe extern "C" fn MacvmRenderAttach(hwnd: isize, pt_tenths: i64, dpi: i64) -> i64 {
    let pt = (pt_tenths.max(1) as f32) / 10.0;
    let dpi = if dpi <= 0 { 96.0 } else { dpi as f32 };
    finish(build(HWND(hwnd as *mut _), pt, dpi).map(|r| R.with(|c| *c.borrow_mut() = Some(r))))
}

/// Release everything. Answers OK even when nothing was attached — detaching
/// twice is not an error, and a pane being destroyed must never fail here.
#[no_mangle]
pub extern "C" fn MacvmRenderDetach() -> i64 {
    R.with(|c| *c.borrow_mut() = None);
    set_error("");
    OK
}

/// `(cell_w << 16) | cell_h`, physical pixels, 0 when not attached.
///
/// One integer because the FFI marshals integers, and these two are small,
/// stable, and always wanted together.
#[no_mangle]
pub extern "C" fn MacvmRenderMetrics() -> i64 {
    R.with(|c| match &*c.borrow() {
        Some(r) => ((r.metrics.cell_w as i64) << 16) | (r.metrics.cell_h as i64),
        None => 0,
    })
}

/// Resize the grid and answer the CELL BUFFER'S ADDRESS, which the guest pokes
/// directly. 0 on failure.
///
/// **The address changes when the dimensions do** — the buffer is reallocated
/// — so the guest must re-ask after a resize and must not cache it across one.
/// Everything the renderer owns it also frees; the guest frees nothing.
#[no_mangle]
pub extern "C" fn MacvmRenderGrid(cols: i64, rows: i64) -> i64 {
    if cols <= 0 || rows <= 0 {
        set_error("grid dimensions must be positive");
        return 0;
    }
    R.with(|c| match &mut *c.borrow_mut() {
        Some(r) => {
            let (cols, rows) = (cols as u32, rows as u32);
            if r.cols != cols || r.rows != rows || r.cells.is_empty() {
                r.cols = cols;
                r.rows = rows;
                r.cells = vec![Cell::blank(); (cols as usize) * (rows as usize)];
            }
            set_error("");
            r.cells.as_mut_ptr() as i64
        }
        None => {
            set_error("no renderer attached");
            0
        }
    })
}

/// Blank every cell to `fg` on `bg`, in Rust.
///
/// The guest could poke all of them, but an 80x40 pane is 3,200 cells and
/// 9,600 marshalled writes per repaint — for a document that is mostly
/// whitespace. Blanking here and letting the guest write only the cells that
/// carry ink keeps the seam's cost proportional to the TEXT rather than to the
/// viewport, which is the whole reason a cell grid is cheap.
#[no_mangle]
pub extern "C" fn MacvmRenderClear(fg: i64, bg: i64) -> i64 {
    R.with(|c| {
        if let Some(r) = &mut *c.borrow_mut() {
            let cell = Cell {
                cp: ' ' as u32,
                fg: fg as u32,
                bg: bg as u32,
            };
            for x in r.cells.iter_mut() {
                *x = cell;
            }
        }
    });
    OK
}

/// The grid's dimensions as `(cols << 16) | rows`, 0 when not attached — so
/// the guest can ask what it got rather than assuming its own arithmetic
/// agreed with the renderer's.
#[no_mangle]
pub extern "C" fn MacvmRenderGridSize() -> i64 {
    R.with(|c| match &*c.borrow() {
        Some(r) => ((r.cols as i64) << 16) | (r.rows as i64),
        None => 0,
    })
}

/// The caret, as a CELL. `on` zero hides it.
///
/// Note the argument order: (col, row). The guest's `editorLineColOf:` answers
/// (line, col), so the one call site that writes this swaps them — and says so,
/// because a caret walking the wrong axis is exactly the kind of defect this
/// port finds by gate rather than by reading.
#[no_mangle]
pub extern "C" fn MacvmRenderSetCaret(col: i64, row: i64, on: i64) -> i64 {
    R.with(|c| {
        if let Some(r) = &mut *c.borrow_mut() {
            r.caret = if on != 0 && col >= 0 && row >= 0 {
                Some((col as u32, row as u32))
            } else {
                None
            };
        }
    });
    OK
}

/// Draw the grid and present it. The guest's whole paint handler.
#[no_mangle]
pub extern "C" fn MacvmRenderPresent() -> i64 {
    finish(R.with(|c| match &mut *c.borrow_mut() {
        Some(r) => r.present(),
        None => Err("no renderer attached".into()),
    }))
}

/// Frames presented since attach — the gate's proof that the pane is being
/// driven at all, in place of GDI's `paintCalls`.
#[no_mangle]
pub extern "C" fn MacvmRenderFrames() -> i64 {
    R.with(|c| match &*c.borrow() {
        Some(r) => r.frames as i64,
        None => 0,
    })
}

/// The last call's reason, UTF-16, NUL-terminated. Empty after a success.
#[no_mangle]
pub extern "C" fn MacvmRenderLastError() -> *const u16 {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Its length EXCLUDING the NUL — the guest reads by count, and one extra unit
/// puts a stray \0 on the end of every transcript line.
#[no_mangle]
pub extern "C" fn MacvmRenderLastErrorLen() -> i64 {
    LAST_ERROR.with(|e| e.borrow().len().saturating_sub(1) as i64)
}
