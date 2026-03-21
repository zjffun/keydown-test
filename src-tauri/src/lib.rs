use base64::{engine::general_purpose::STANDARD, Engine};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Emitter;

// ── Types ────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct ScreenshotResult {
    image: String,
    width: u32,
    height: u32,
}

#[derive(Serialize, Clone)]
struct CropResult {
    image: String,
}

#[derive(Deserialize)]
struct CropRegion {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Serialize, Clone)]
struct TabCapture {
    avatar_image: String,
}

#[derive(Serialize, Clone)]
struct CaptureError {
    message: String,
}

// ── Screen capture ───────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn capture_full_screen() -> Result<RgbaImage, String> {
    use std::mem;
    use std::ptr::null_mut;
    use winapi::um::wingdi::*;
    use winapi::um::winuser::*;

    unsafe {
        let hdc = GetDC(null_mut());
        if hdc.is_null() {
            return Err("GetDC 失败".into());
        }

        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w <= 0 || h <= 0 {
            ReleaseDC(null_mut(), hdc);
            return Err("屏幕尺寸异常".into());
        }

        let hdc_mem = CreateCompatibleDC(hdc);
        let hbm = CreateCompatibleBitmap(hdc, w, h);
        if hdc_mem.is_null() || hbm.is_null() {
            if !hdc_mem.is_null() { DeleteDC(hdc_mem); }
            if !hbm.is_null() { DeleteObject(hbm as *mut _); }
            ReleaseDC(null_mut(), hdc);
            return Err("GDI 资源创建失败".into());
        }

        let old = SelectObject(hdc_mem, hbm as *mut _);
        BitBlt(hdc_mem, 0, 0, w, h, hdc, 0, 0, SRCCOPY);

        let mut bmi: BITMAPINFO = mem::zeroed();
        bmi.bmiHeader.biSize = mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut px = vec![0u8; (w * h * 4) as usize];
        let ok = GetDIBits(
            hdc_mem, hbm, 0, h as u32,
            px.as_mut_ptr() as *mut _,
            &mut bmi, DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old);
        DeleteObject(hbm as *mut _);
        DeleteDC(hdc_mem);
        ReleaseDC(null_mut(), hdc);

        if ok == 0 {
            return Err("GetDIBits 失败".into());
        }

        // BGRA → RGBA
        for c in px.chunks_exact_mut(4) { c.swap(0, 2); }

        RgbaImage::from_raw(w as u32, h as u32, px).ok_or("图像创建失败".into())
    }
}

#[cfg(not(target_os = "windows"))]
fn capture_full_screen() -> Result<RgbaImage, String> {
    let monitors = xcap::Monitor::all().map_err(|e| format!("枚举显示器失败: {}", e))?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or("未找到主显示器")?;
    monitor
        .capture_image()
        .map_err(|e| format!("截屏失败: {}", e))
}

// ── F6 key listener (Windows: polling, macOS: global shortcut) ──────

#[cfg(target_os = "windows")]
fn start_f6_listener(handle: tauri::AppHandle, capturing: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut was_pressed = false;
        loop {
            let state = unsafe { winapi::um::winuser::GetAsyncKeyState(0x75) }; // VK_F6
            let is_pressed = (state & (1i16 << 15)) != 0;

            if is_pressed && !was_pressed {
                let _ = handle.emit("f6-pressed", ());

                if !capturing.load(Ordering::SeqCst) {
                    let h = handle.clone();
                    let flag = Arc::clone(&capturing);
                    flag.store(true, Ordering::SeqCst);

                    std::thread::spawn(move || {
                        match capture_crop_impl() {
                            Ok(cap) => { let _ = h.emit("tab-captured", cap); }
                            Err(e) => { let _ = h.emit("capture-error", CaptureError { message: e }); }
                        }
                        flag.store(false, Ordering::SeqCst);
                    });
                }
            }
            was_pressed = is_pressed;
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    });
}


// ── Saved region (set from frontend) ─────────────────────────────────

use std::sync::Mutex;

static SAVED_REGION: once_cell::sync::Lazy<Mutex<Option<(u32, u32, u32, u32)>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

// ── Commands ─────────────────────────────────────────────────────────

/// Take a screenshot and return a downscaled base64 image for the selection overlay.
#[tauri::command]
fn take_screenshot() -> Result<ScreenshotResult, String> {
    let screen = capture_full_screen()?;
    let (sw, sh) = screen.dimensions();

    // Downscale 2× for the overlay (smaller transfer)
    let ds = 2u32;
    let thumb = image::imageops::resize(
        &screen, sw / ds, sh / ds, image::imageops::FilterType::Triangle,
    );
    let image = encode_to_data_url(&thumb)?;

    Ok(ScreenshotResult { image, width: sw, height: sh })
}

/// Save the user-selected crop region (in actual screen coordinates).
#[tauri::command]
fn save_region(region: CropRegion) -> Result<(), String> {
    let mut saved = SAVED_REGION.lock().map_err(|e| format!("lock: {}", e))?;
    *saved = Some((region.x, region.y, region.w, region.h));
    Ok(())
}

/// Crop the saved region from a fresh screenshot and return base64.
#[tauri::command]
fn crop_screen() -> Result<CropResult, String> {
    let region = {
        let saved = SAVED_REGION.lock().map_err(|e| format!("lock: {}", e))?;
        saved.ok_or("尚未框选区域")?
    };

    let screen = capture_full_screen()?;
    let (sw, sh) = screen.dimensions();
    let (x, y, w, h) = region;

    let cw = w.min(sw.saturating_sub(x));
    let ch = h.min(sh.saturating_sub(y));
    if cw < 2 || ch < 2 {
        return Err("裁剪区域太小".into());
    }

    let crop = image::imageops::crop_imm(&screen, x, y, cw, ch).to_image();
    let image = encode_to_data_url(&crop)?;
    Ok(CropResult { image })
}

/// Used by F6 to capture with saved region.
fn capture_crop_impl() -> Result<TabCapture, String> {
    let result = crop_screen()?;
    Ok(TabCapture { avatar_image: result.image })
}

// ── Utilities ────────────────────────────────────────────────────────

fn encode_to_data_url(img: &RgbaImage) -> Result<String, String> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| format!("编码失败: {}", e))?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(&buf)))
}

// ── App entry ────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![take_screenshot, save_region, crop_screen])
        .setup(|app| {
            let handle = app.handle().clone();
            #[cfg(target_os = "windows")]
            {
                let capturing = Arc::new(AtomicBool::new(false));
                start_f6_listener(handle, capturing);
            }
            #[cfg(not(target_os = "windows"))]
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

                let shortcut = Shortcut::new(None, Code::F6);
                let capturing = Arc::new(AtomicBool::new(false));

                app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        let _ = handle.emit("f6-pressed", ());

                        if !capturing.load(Ordering::SeqCst) {
                            let h = handle.clone();
                            let flag = Arc::clone(&capturing);
                            flag.store(true, Ordering::SeqCst);

                            std::thread::spawn(move || {
                                match capture_crop_impl() {
                                    Ok(cap) => { let _ = h.emit("tab-captured", cap); }
                                    Err(e) => { let _ = h.emit("capture-error", CaptureError { message: e }); }
                                }
                                flag.store(false, Ordering::SeqCst);
                            });
                        }
                    }
                }).map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
