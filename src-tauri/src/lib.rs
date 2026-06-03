use base64::{engine::general_purpose::STANDARD, Engine};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tauri::{Emitter, Manager, PhysicalPosition};

// ── Shared state ─────────────────────────────────────────────────────

/// True while a capture is in flight (prevents overlapping captures).
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Currently configured listen key as an F-number (1–9). Defaults to F4.
static LISTEN_KEY: AtomicU8 = AtomicU8::new(4);

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

// ── Listen key (Windows: polling, macOS: global shortcut) ───────────

/// Emit the press event and kick off a capture (skipped if one is in flight).
fn run_capture(handle: tauri::AppHandle) {
    let _ = handle.emit("listen-key-pressed", ());

    // swap returns the previous value; bail if a capture is already running.
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(move || {
        match capture_crop_impl() {
            Ok(cap) => { let _ = handle.emit("tab-captured", cap); }
            Err(e) => { let _ = handle.emit("capture-error", CaptureError { message: e }); }
        }
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

/// Map an F-number (1–9) to its Windows virtual-key code (VK_F1 = 0x70).
#[cfg(target_os = "windows")]
fn vk_for_fkey(n: u8) -> i32 {
    0x70 + (n.clamp(1, 9) as i32 - 1)
}

/// Map an F-number (1–9) to the global-shortcut key code (macOS/Linux).
#[cfg(not(target_os = "windows"))]
fn code_for_fkey(n: u8) -> Option<tauri_plugin_global_shortcut::Code> {
    use tauri_plugin_global_shortcut::Code;
    Some(match n {
        1 => Code::F1,
        2 => Code::F2,
        3 => Code::F3,
        4 => Code::F4,
        5 => Code::F5,
        6 => Code::F6,
        7 => Code::F7,
        8 => Code::F8,
        9 => Code::F9,
        _ => return None,
    })
}

/// Poll the configured key on Windows; LISTEN_KEY is re-read each tick so
/// config changes take effect live.
#[cfg(target_os = "windows")]
fn start_key_listener(handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut was_pressed = false;
        loop {
            let vk = vk_for_fkey(LISTEN_KEY.load(Ordering::SeqCst));
            let state = unsafe { winapi::um::winuser::GetAsyncKeyState(vk) };
            let is_pressed = (state & (1i16 << 15)) != 0;

            if is_pressed && !was_pressed {
                run_capture(handle.clone());
            }
            was_pressed = is_pressed;
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    });
}

/// Register the global shortcut for the given F-number (macOS/Linux).
#[cfg(not(target_os = "windows"))]
fn register_listen_shortcut(handle: &tauri::AppHandle, n: u8) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    let code = code_for_fkey(n).ok_or("无效的按键")?;
    let shortcut = Shortcut::new(None, code);
    let h = handle.clone();
    handle
        .global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                run_capture(h.clone());
            }
        })
        .map_err(|e| e.to_string())
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

/// Set which F-key (1–9) triggers a capture; re-registers the shortcut live.
#[tauri::command]
#[cfg_attr(target_os = "windows", allow(unused_variables))]
fn set_listen_key(app: tauri::AppHandle, key: u8) -> Result<(), String> {
    if !(1..=9).contains(&key) {
        return Err("无效的按键，仅支持 F1–F9".into());
    }

    let old = LISTEN_KEY.swap(key, Ordering::SeqCst);

    // Windows reads LISTEN_KEY in its polling loop, so nothing else to do there.
    #[cfg(not(target_os = "windows"))]
    if old != key {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
        if let Some(code) = code_for_fkey(old) {
            let _ = app.global_shortcut().unregister(Shortcut::new(None, code));
        }
        register_listen_shortcut(&app, key)?;
    }

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

/// Used by the listen key to capture with the saved region.
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
        .invoke_handler(tauri::generate_handler![
            take_screenshot,
            save_region,
            crop_screen,
            set_listen_key
        ])
        .setup(|app| {
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(Some(monitor)) = win.primary_monitor() {
                    let m_size = monitor.size();
                    let m_pos = monitor.position();
                    if let Ok(w_size) = win.outer_size() {
                        let x = m_pos.x + m_size.width as i32 - w_size.width as i32;
                        let y = m_pos.y;
                        let _ = win.set_position(PhysicalPosition::new(x, y));
                    }
                }
            }

            let handle = app.handle().clone();
            #[cfg(target_os = "windows")]
            start_key_listener(handle);
            #[cfg(not(target_os = "windows"))]
            register_listen_shortcut(&handle, LISTEN_KEY.load(Ordering::SeqCst))?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
