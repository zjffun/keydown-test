use tauri::Emitter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut was_pressed = false;
                loop {
                    let state = unsafe { winapi::um::winuser::GetAsyncKeyState(0x75) }; // 0x75 = VK_F6
                    let is_pressed = (state & (1i16 << 15)) != 0; // high bit = currently pressed

                    if is_pressed && !was_pressed {
                        let _ = handle.emit("f6-pressed", ());
                    }
                    was_pressed = is_pressed;

                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
