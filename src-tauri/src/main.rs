// 正式版不跳出黑色主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{
    ipc::Response,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    window::Monitor,
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

/// 長條視窗的高度（邏輯像素，會依螢幕縮放比例換算）
const STRIP_HEIGHT: f64 = 150.0;

/// 雷射光、逗貓棒跟著游標跑遍全螢幕，要一直回報游標位置；
/// 毛球只在乎游標有沒有在長條裡，離開長條就不用回報
static FOLLOW_EVERYWHERE: AtomicBool = AtomicBool::new(false);

/// 系統匣「螢幕」選的螢幕（用 monitor_key 辨識）；None 表示主螢幕
static CHOSEN_SCREEN: Mutex<Option<String>> = Mutex::new(None);

/// 選單換了螢幕，請監看執行緒馬上重擺，不用等下一次的每秒檢查
static PLACE_NOW: AtomicBool = AtomicBool::new(false);

/// 每隔幾格（每格 16ms）檢查一次長條的位置，約 1 秒
const PLACE_CHECK_TICKS: u32 = 60;

/// 同一個目標位置最多補擺幾次，避免系統不肯配合時每秒都在搬
const PLACE_RETRIES: u8 = 3;

/// 前端判斷游標在不在貓或毛球上之後，呼叫這個開關「滑鼠穿透」
#[tauri::command]
fn set_passthrough(window: WebviewWindow, on: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(on).map_err(|e| e.to_string())
}

/// 大泡泡的圖片放在「圖片」資料夾底下的這個資料夾（Windows：圖片\YarnCat，macOS：~/Pictures/YarnCat）
const PICTURE_FOLDER: &str = "YarnCat";

/// 大泡泡用得了的圖片（副檔名，小寫）：Windows、macOS 的 WebView 都看得懂的格式
const PICTURE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// 檔案太大的圖片不讀：解開來太吃記憶體
const PICTURE_MAX_BYTES: u64 = 20 * 1024 * 1024;

fn picture_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().picture_dir().ok().map(|d| d.join(PICTURE_FOLDER))
}

/// 大泡泡用得了嗎：副檔名對、是檔案、不會太大
fn is_picture(path: &Path) -> bool {
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PICTURE_EXTS.contains(&e.to_ascii_lowercase().as_str()));
    ext_ok && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= PICTURE_MAX_BYTES)
}

/// 圖片資料夾裡有哪些圖片，只回傳檔名；資料夾還沒建立就是一張都沒有
#[tauri::command]
async fn list_pictures(app: AppHandle) -> Vec<String> {
    let Some(dir) = picture_dir(&app) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| is_picture(&e.path()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.')) // macOS 在某些磁碟上會多出 ._ 開頭的隱藏檔
        .collect()
}

/// 讀一張圖片給前端畫。只收 list_pictures 給的檔名，帶路徑的（..、斜線）一律不讀
#[tauri::command]
async fn read_picture(app: AppHandle, name: String) -> Result<Response, String> {
    let dir = picture_dir(&app).ok_or("找不到圖片資料夾")?;
    if Path::new(&name).file_name() != Some(OsStr::new(&name)) {
        return Err(format!("不是檔名：{name}"));
    }
    let path = dir.join(&name);
    if !is_picture(&path) {
        return Err(format!("不是用得了的圖片：{name}"));
    }
    std::fs::read(&path).map(Response::new).map_err(|e| e.to_string())
}

/// 系統匣「打開泡泡圖片資料夾」：還沒有就先建立，再用檔案總管（macOS 是 Finder）打開
fn open_picture_dir(app: &AppHandle) {
    let Some(dir) = picture_dir(app) else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let opener = "xdg-open";
    // 等它結束才不會留下殭屍行程；放到別的執行緒等，選單才不會卡住
    thread::spawn(move || {
        let _ = Command::new(opener).arg(dir).status();
    });
}

/// 長條該擺在哪、多大（實體像素）：(x, y, 寬, 高)
type Rect = (i32, i32, u32, u32);

/// 辨識螢幕用的名字（Windows 是 \\.\DISPLAY1 這類）；拿不到名字就用左上角座標
fn monitor_key(m: &Monitor) -> String {
    match m.name() {
        Some(n) => n.clone(),
        None => format!("{},{}", m.position().x, m.position().y),
    }
}

/// 貓要待的螢幕：系統匣選的那個；沒選、或那個螢幕已經拔掉了，就用主螢幕
fn pick_monitor(win: &WebviewWindow) -> tauri::Result<Option<Monitor>> {
    let chosen = CHOSEN_SCREEN.lock().ok().and_then(|c| c.clone());
    if let Some(key) = chosen {
        let found = win
            .available_monitors()?
            .into_iter()
            .find(|m| monitor_key(m) == key);
        if found.is_some() {
            return Ok(found);
        }
    }
    match win.primary_monitor()? {
        Some(m) => Ok(Some(m)),
        None => win.current_monitor(),
    }
}

/// 算出長條的位置：所選螢幕的底部、工作列正上方
fn strip_rect(win: &WebviewWindow) -> tauri::Result<Option<Rect>> {
    let Some(monitor) = pick_monitor(win)? else {
        return Ok(None);
    };
    // work_area = 螢幕扣掉工作列的範圍，工作列放在哪一邊都適用
    let area = monitor.work_area();
    let height = (STRIP_HEIGHT * monitor.scale_factor()).round() as u32;
    let bottom = area.position.y + area.size.height as i32;
    Ok(Some((area.position.x, bottom - height as i32, area.size.width, height)))
}

/// 把視窗擺到 rect。
/// 先搬位置再改大小：搬到縮放比例不同的螢幕時，Windows 會自動依新比例縮放視窗，
/// 最後再設一次大小，才會是我們算好的尺寸。
fn apply_rect(win: &WebviewWindow, r: Rect) -> tauri::Result<()> {
    win.set_position(PhysicalPosition::new(r.0, r.1))?;
    win.set_size(PhysicalSize::new(r.2, r.3))?;
    Ok(())
}

/// 視窗現在實際在哪、多大
fn actual_rect(win: &WebviewWindow) -> Option<Rect> {
    let p = win.outer_position().ok()?;
    let s = win.inner_size().ok()?;
    Some((p.x, p.y, s.width, s.height))
}

/// 換螢幕、改縮放、搬工作列之後，長條要跟著重擺。
/// 這些變化不一定有事件可聽，所以定期比對「該在的位置」和「實際位置」。
struct Placer {
    want: Option<Rect>,
    retries: u8,
}

impl Placer {
    fn check(&mut self, win: &WebviewWindow) {
        let Ok(Some(want)) = strip_rect(win) else {
            return;
        };
        if actual_rect(win) == Some(want) {
            self.want = Some(want);
            self.retries = 0;
            return;
        }
        if self.want == Some(want) {
            if self.retries >= PLACE_RETRIES {
                return;
            }
            self.retries += 1;
        } else {
            self.want = Some(want);
            self.retries = 1;
        }
        let _ = apply_rect(win, want);
    }
}

/// 游標相對於長條左上角的位置，單位是實體像素。
/// 不在這裡用視窗的 scale_factor 換成 CSS 像素：切到縮放比例不同的螢幕後，
/// 它和 WebView 實際用的倍率不一定一致（WebView2 還會乘上 Windows 的「文字大小」），
/// 一不一致，紅點、逗貓棒就會跟游標等比例錯開。換算交給前端用自己的 devicePixelRatio。
#[derive(Clone, Serialize)]
struct Cursor {
    x: f64,
    y: f64,
}

/// 視窗穿透時收不到任何滑鼠事件，所以由這裡主動輪詢游標位置，轉成相對於長條的位置交給前端。
/// 順便每秒檢查一次長條有沒有擺對位置。
fn watch_cursor(win: WebviewWindow) {
    thread::spawn(move || {
        let mut last = (f64::NAN, f64::NAN);
        let mut was_inside = false;
        let mut placer = Placer { want: None, retries: 0 };
        let mut strip = actual_rect(&win);
        let mut tick: u32 = 0;
        loop {
            thread::sleep(Duration::from_millis(16));
            tick = tick.wrapping_add(1);
            // 視窗位置、大小只有重擺時才會變，每秒讀一次就好，不用每格都問
            let now = PLACE_NOW.swap(false, Ordering::Relaxed);
            if now || tick % PLACE_CHECK_TICKS == 0 {
                placer.check(&win);
                strip = actual_rect(&win);
            }
            let Some((left, top, w, h)) = strip else {
                continue;
            };
            // 實體像素，不除縮放比例（見 Cursor 的說明）
            let Some((x, y)) = cursor_in_strip(&win, (left, top)) else {
                continue;
            };
            let inside = x >= 0.0 && x < w as f64 && y >= 0.0 && y < h as f64;
            // 毛球模式：游標在長條外就不回報，只在剛離開時補一次，讓前端把穿透打開
            let report = FOLLOW_EVERYWHERE.load(Ordering::Relaxed) || inside || was_inside;
            was_inside = inside;
            if !report {
                continue;
            }
            if (x, y) == last {
                continue;
            }
            last = (x, y);
            let _ = win.emit("cursor", Cursor { x, y });
        }
    });
}

/// 游標相對於長條左上角的位置，單位是長條所在螢幕的實體像素。
/// Windows 的游標、視窗座標都是實體像素，直接相減就好。
#[cfg(not(target_os = "macos"))]
fn cursor_in_strip(win: &WebviewWindow, (left, top): (i32, i32)) -> Option<(f64, f64)> {
    let c = win.cursor_position().ok()?;
    Some((c.x - left as f64, c.y - top as f64))
}

/// macOS 不能用 cursor_position：tao 把游標座標乘上「主螢幕」的縮放比例，
/// 視窗位置卻乘上「視窗所在螢幕」的比例（tao 0.37 macos/util/mod.rs 的 cursor_position；tauri#7890）。
/// 長條放在縮放比例跟主螢幕不同的螢幕時，兩邊單位不同，紅點會差好幾百點。
/// 所以直接問 AppKit 游標在這個視窗裡的哪裡，完全不經過 tao 的換算。
/// AppKit 的視窗只能在主執行緒碰，丟到主執行緒做完再把結果帶回來。
#[cfg(target_os = "macos")]
fn cursor_in_strip(win: &WebviewWindow, _origin: (i32, i32)) -> Option<(f64, f64)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let w = win.clone();
    win.run_on_main_thread(move || {
        let pos = w
            .ns_view()
            .ok()
            .and_then(|view| unsafe { appkit::cursor_in_window(view.cast()) });
        let _ = tx.send(pos);
    })
    .ok()?;
    rx.recv().ok().flatten()
}

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::encode::{Encode, Encoding};
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    // AppKit 回傳的結構，欄位要照 C 的排列（有些欄位用不到）。macOS 只有 64 位元，CGFloat 就是 f64
    #[allow(dead_code)]
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    unsafe impl Encode for CGPoint {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    unsafe impl Encode for CGSize {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }
    unsafe impl Encode for CGRect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
    }

    /// 游標相對於視窗左上角的位置，單位是視窗所在螢幕的實體像素。
    /// view 是視窗的內容 view；只能在主執行緒呼叫。
    pub unsafe fn cursor_in_window(view: *mut AnyObject) -> Option<(f64, f64)> {
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return None;
        }
        // AppKit 的視窗座標原點在左下角、往上為正，單位是點。長條沒有標題列，視窗外框就是內容範圍
        let p: CGPoint = msg_send![window, mouseLocationOutsideOfEventStream];
        let frame: CGRect = msg_send![window, frame];
        let scale: f64 = msg_send![window, backingScaleFactor];
        Some((p.x * scale, (frame.size.height - p.y) * scale))
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_passthrough, list_pictures, read_picture])
        .setup(|app| {
            // macOS：不在 Dock 出現，只留選單列的貓咪圖示
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let win = app
                .get_webview_window("main")
                .expect("找不到 main 視窗");
            if let Some(r) = strip_rect(&win)? {
                apply_rect(&win, r)?;
            }
            win.set_ignore_cursor_events(true)?;
            win.show()?;
            watch_cursor(win.clone());

            // 系統匣：換玩具、換螢幕、打開泡泡圖片資料夾、安靜模式（開會、分享螢幕時讓貓去睡）、結束
            let toy_ball = CheckMenuItem::with_id(app, "toy:ball", "毛球", true, true, None::<&str>)?;
            let toy_laser = CheckMenuItem::with_id(app, "toy:laser", "雷射光", true, false, None::<&str>)?;
            let toy_wand = CheckMenuItem::with_id(app, "toy:wand", "逗貓棒", true, false, None::<&str>)?;
            let toys = Submenu::with_items(app, "玩具", true, &[&toy_ball, &toy_laser, &toy_wand])?;
            // 螢幕：啟動當下接著的螢幕，由左到右編號。之後才接上的螢幕要重開毛球貓才會出現
            let mut monitors = win.available_monitors()?;
            monitors.sort_by_key(|m| (m.position().x, m.position().y));
            let primary_key = win.primary_monitor()?.map(|m| monitor_key(&m));
            let mut screen_items = Vec::new();
            let mut screen_keys = Vec::new();
            for (i, m) in monitors.iter().enumerate() {
                let key = monitor_key(m);
                let is_primary = primary_key.as_deref() == Some(key.as_str());
                let label = format!(
                    "螢幕 {}：{}×{}{}",
                    i + 1,
                    m.size().width,
                    m.size().height,
                    if is_primary { "（主螢幕）" } else { "" }
                );
                let id = format!("screen:{i}");
                screen_items.push(CheckMenuItem::with_id(app, id, label, true, is_primary, None::<&str>)?);
                screen_keys.push(key);
            }
            let screen_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
                screen_items.iter().map(|it| it as &dyn IsMenuItem<tauri::Wry>).collect();
            let screens = Submenu::with_items(app, "螢幕", true, &screen_refs)?;
            let pictures = MenuItem::with_id(app, "pictures", "打開泡泡圖片資料夾", true, None::<&str>)?;
            let quiet = CheckMenuItem::with_id(app, "quiet", "安靜模式", true, false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束毛球貓", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toys, &screens, &pictures, &quiet, &quit])?;
            let quiet_item = quiet.clone();
            let toy_items = [toy_ball, toy_laser, toy_wand];
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().expect("缺少圖示").clone())
                .tooltip("毛球貓")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quiet" => {
                        let on = quiet_item.is_checked().unwrap_or(false);
                        let _ = app.emit("quiet", on);
                    }
                    "pictures" => open_picture_dir(app),
                    "quit" => app.exit(0),
                    // 玩具只能選一個：點到的打勾，其他取消
                    id if id.starts_with("toy:") => {
                        for item in &toy_items {
                            let mine: &str = item.id().as_ref();
                            let _ = item.set_checked(mine == id);
                        }
                        FOLLOW_EVERYWHERE.store(id != "toy:ball", Ordering::Relaxed);
                        let _ = app.emit("toy", &id[4..]);
                    }
                    // 螢幕也只能選一個；選好之後請監看執行緒馬上把長條搬過去
                    id if id.starts_with("screen:") => {
                        for item in &screen_items {
                            let mine: &str = item.id().as_ref();
                            let _ = item.set_checked(mine == id);
                        }
                        let picked = id[7..].parse::<usize>().ok().and_then(|i| screen_keys.get(i));
                        if let (Some(key), Ok(mut chosen)) = (picked, CHOSEN_SCREEN.lock()) {
                            *chosen = Some(key.clone());
                            PLACE_NOW.store(true, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("毛球貓啟動失敗");
}
