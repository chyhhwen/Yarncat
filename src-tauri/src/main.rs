// 正式版不跳出黑色主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod link;

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use link::Link;
use serde::{Deserialize, Serialize};
use tauri::{
    ipc::Response,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::Monitor,
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

/// 長條視窗的高度（邏輯像素，會依螢幕縮放比例換算）
const STRIP_HEIGHT: f64 = 150.0;

/// 雷射光、逗貓棒跟著游標跑遍全螢幕，要一直回報游標位置；
/// 毛球只在乎游標有沒有在長條裡，離開長條就不用回報
static FOLLOW_EVERYWHERE: AtomicBool = AtomicBool::new(false);

/// 狀態視窗「螢幕」選的螢幕（用 monitor_key 辨識）；None 表示主螢幕。開機時從設定檔讀進來
static CHOSEN_SCREEN: Mutex<Option<String>> = Mutex::new(None);

/// 換了螢幕，請監看執行緒馬上重擺，不用等下一次的每秒檢查
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

/* ─── 設定：會記住的只有毛色、家具、貓待在哪個螢幕、串門子的連線設定、要不要記貓咪日記 ─── */

/// 存在 app 設定資料夾的 settings.json（Windows 是 %APPDATA%\com.yarncat.desktop；
/// 商店版會被 Windows 導到 app 自己的資料夾，解除安裝時一起清掉）。
/// 玩具、安靜模式是「現在在玩什麼」，不記：重開回到毛球、不安靜。
/// 讀不到或格式不對就用預設值，設定檔壞掉也開得起來
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// 毛色；None 是沒選過（藍灰，串門子撞色時可以自動換一隻）
    coat: Option<String>,
    furniture: Furniture,
    /// 貓待在哪個螢幕（monitor_key）；None 是主螢幕
    screen: Option<String>,
    link: LinkSettings,
    /// 記不記貓咪日記（見「貓咪日記」）。預設打開：只記次數和時間，存在自己的電腦上
    diary: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { coat: None, furniture: Furniture::default(), screen: None, link: LinkSettings::default(), diary: true }
    }
}

/// 串門子的連線設定（狀態視窗的「連線」分頁）
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct LinkSettings {
    /// 「連線」開關。預設關閉：打開了才會開 port、在區網上廣播電腦名稱
    on: bool,
    /// 這台電腦的代號，第一次開毛球貓時產生；另一台靠它記得勾的是哪一台
    id: String,
    /// 勾選的那台；兩邊互相勾選才會連上
    peer: Option<LinkPeer>,
    /// 隔壁在我的哪一邊："left"、"right"。None 是自動：比兩台的代號，一台算右邊、另一台就算左邊
    side: Option<String>,
    /// 手動輸入的 IP：不同網段時廣播傳不過去
    ips: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct LinkPeer {
    id: String,
    name: String,
}

/// 手動輸入的 IP 最多記幾個
const LINK_MAX_IPS: usize = 8;

/// 家具擺不擺出來；預設全部擺出來
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Furniture {
    litter: bool,
    tree: bool,
    r#box: bool,
}

impl Default for Furniture {
    fn default() -> Self {
        Furniture { litter: true, tree: true, r#box: true }
    }
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 先寫到暫存檔再改名，寫到一半當掉也不會留下壞掉的設定檔
fn save_settings(app: &AppHandle, s: &Settings) {
    let Some(path) = settings_path(app) else {
        return;
    };
    let Ok(text) = serde_json::to_string_pretty(s) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 改設定：存檔，再通知所有視窗（貓的視窗照著擺，狀態視窗更新畫面）
fn update_settings(app: &AppHandle, change: impl FnOnce(&mut Settings)) {
    let state = app.state::<Mutex<Settings>>();
    let snapshot = {
        let Ok(mut s) = state.lock() else {
            return;
        };
        change(&mut s);
        s.clone()
    };
    save_settings(app, &snapshot);
    let _ = app.emit("settings", &snapshot);
}

fn settings_snapshot(app: &AppHandle) -> Settings {
    let state = app.state::<Mutex<Settings>>();
    let settings = state.lock().map(|s| s.clone()).unwrap_or_default();
    settings
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    settings_snapshot(&app)
}

/// 毛色的名字只收小寫英文字母（gray、orange…）。有哪些毛色由前端的 COATS 決定，這裡不重複列一份；
/// 前端讀到不認得的毛色會照預設的藍灰畫
#[tauri::command]
fn set_coat(app: AppHandle, coat: String) -> Result<(), String> {
    if coat.is_empty() || coat.len() > 20 || !coat.chars().all(|c| c.is_ascii_lowercase()) {
        return Err(format!("毛色的名字不對：{coat}"));
    }
    update_settings(&app, |s| s.coat = Some(coat));
    Ok(())
}

#[tauri::command]
fn set_furniture(app: AppHandle, name: String, on: bool) -> Result<(), String> {
    if !["litter", "tree", "box"].contains(&name.as_str()) {
        return Err(format!("沒有這件家具：{name}"));
    }
    update_settings(&app, |s| match name.as_str() {
        "litter" => s.furniture.litter = on,
        "tree" => s.furniture.tree = on,
        _ => s.furniture.r#box = on,
    });
    Ok(())
}

/// 狀態視窗「螢幕」分頁的一個選項
#[derive(Serialize)]
struct Screen {
    key: String,
    label: String,
    chosen: bool,
}

/// 現在接著的螢幕，由左到右編號。每次打開分頁都重新問，剛接上的螢幕不用重開毛球貓就會出現
#[tauri::command]
fn list_screens(app: AppHandle) -> Result<Vec<Screen>, String> {
    let mut monitors = app.available_monitors().map_err(|e| e.to_string())?;
    monitors.sort_by_key(|m| (m.position().x, m.position().y));
    let primary = app.primary_monitor().ok().flatten().map(|m| monitor_key(&m));
    // 選的螢幕拔掉了，貓會待在主螢幕上（見 pick_monitor），這裡也勾主螢幕
    let chosen = CHOSEN_SCREEN
        .lock()
        .ok()
        .and_then(|c| c.clone())
        .filter(|k| monitors.iter().any(|m| monitor_key(m) == *k))
        .or_else(|| primary.clone());
    Ok(monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let key = monitor_key(m);
            let is_primary = primary.as_deref() == Some(key.as_str());
            Screen {
                label: format!(
                    "螢幕 {}：{}×{}{}",
                    i + 1,
                    m.size().width,
                    m.size().height,
                    if is_primary { "（主螢幕）" } else { "" }
                ),
                chosen: chosen.as_deref() == Some(key.as_str()),
                key,
            }
        })
        .collect())
}

/// 換螢幕：記下來，再請監看執行緒馬上把長條搬過去
#[tauri::command]
fn set_screen(app: AppHandle, key: String) -> Result<(), String> {
    let exists = app
        .available_monitors()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|m| monitor_key(m) == key);
    if !exists {
        return Err(format!("找不到這個螢幕：{key}"));
    }
    if let Ok(mut chosen) = CHOSEN_SCREEN.lock() {
        *chosen = Some(key.clone());
    }
    PLACE_NOW.store(true, Ordering::Relaxed);
    update_settings(&app, |s| s.screen = Some(key));
    Ok(())
}

/* ─── 貓咪日記：外傳「午睡夢遊記」讀它，讓白天玩了什麼影響晚上的夢 ───
   放在圖片\YarnCat\diary.json，不放 app 的設定資料夾：商店版（MSIX）寫進 AppData 的東西
   會被 Windows 導到 app 自己的資料夾，外傳讀不到；「圖片」不會被導走。
   只記每天的次數和秒數，不記游標、不記圖片。記什麼由前端決定（index.html 的「貓咪日記」），
   這裡不重複列一份，只檢查名字和數字像不像樣。

   {
     "version": 1,
     "days": { "2026-10-23": { "pets": 12, "purr_sec": 95, "toy_laser_sec": 340, ... } }
   }
   欄位只會多不會少；外傳讀到不認得的欄位略過、缺的當 0。 */

/// 日記格式的版本：之後改了欄位的意思才加一，只是多了欄位不用改
const DIARY_VERSION: u32 = 1;

/// 只留最近幾天：外傳看的是最近幾天的累積
const DIARY_DAYS: usize = 30;

/// 一次最多加多少：前端一分鐘送一次，秒數不會超過 60，次數也不會多到哪去
const DIARY_MAX_ADD: u64 = 3600;

/// 讀、改、寫一次做完，兩次寫入不會互相蓋掉
static DIARY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Diary {
    version: u32,
    /// 日期（YYYY-MM-DD，本地時間）→ 欄位 → 次數或秒數。BTreeMap 照日期排好，剪掉舊的就是剪最前面
    days: BTreeMap<String, BTreeMap<String, u64>>,
}

fn diary_path(app: &AppHandle) -> Option<PathBuf> {
    picture_dir(app).map(|d| d.join("diary.json"))
}

/// 日期要是 YYYY-MM-DD
fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// 欄位名稱只收小寫英文、數字、底線
fn valid_field(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// 前端每分鐘送一次這一分鐘多出來的次數、秒數，加進那一天。
/// 寫不進去就回傳錯誤，前端會留著下一次再送（例如 OneDrive 正在同步這個檔案）
#[tauri::command]
async fn diary_add(app: AppHandle, day: String, add: HashMap<String, u64>) -> Result<(), String> {
    if !settings_snapshot(&app).diary {
        return Ok(()); // 剛關掉，還在路上的那一筆不寫
    }
    if !valid_day(&day) {
        return Err(format!("日期不對：{day}"));
    }
    if let Some((k, _)) = add.iter().find(|(k, v)| !valid_field(k) || **v > DIARY_MAX_ADD) {
        return Err(format!("欄位不對：{k}"));
    }
    let path = diary_path(&app).ok_or("找不到圖片資料夾")?;
    let _lock = DIARY_LOCK.lock().map_err(|e| e.to_string())?;
    // 讀不到或格式壞了就從頭記：日記只是驚喜，不值得因為它開不起來
    let mut diary: Diary = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    diary.version = DIARY_VERSION;
    let today = diary.days.entry(day).or_default();
    for (k, v) in add {
        let n = today.entry(k).or_default();
        *n = n.saturating_add(v);
    }
    while diary.days.len() > DIARY_DAYS {
        diary.days.pop_first();
    }
    let text = serde_json::to_string_pretty(&diary).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // 先寫到暫存檔再改名（同 save_settings），外傳剛好在讀也不會讀到寫一半的檔案
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_diary(app: AppHandle, on: bool) {
    update_settings(&app, |s| s.diary = on);
}

/* ─── 串門子：區網連線在 link.rs，這裡接上設定和兩個視窗 ───
   貓的視窗（index.html）：收 link（隔壁的訊息）、link-conn（連上沒、隔壁在哪一邊），用 link_send 送出。
   狀態視窗（status.html）的「連線」分頁：收 link-status，用下面的 set_link_* 改設定。 */

/// 貓的視窗要知道的：連上了沒、隔壁在哪一邊（1 右邊、-1 左邊）
#[derive(Clone, Serialize)]
struct LinkConn {
    up: bool,
    side: i8,
}

/// 「連線」分頁要顯示的：link.rs 的狀態，再加上隔壁在哪一邊（自動的話要算出來才知道勾哪個）
#[derive(Clone, Serialize)]
struct LinkView {
    #[serde(flatten)]
    status: link::Status,
    side: i8,
}

/// 隔壁在哪一邊：狀態視窗選的那邊；沒選就比代號，
/// 小的那台說隔壁在右邊、大的那台說在左邊，貓從這台右邊走出去，就從那台左邊走進來
fn link_side(app: &AppHandle) -> i8 {
    let s = settings_snapshot(app).link;
    match s.side.as_deref() {
        Some("left") => -1,
        Some("right") => 1,
        _ => match &s.peer {
            Some(p) if p.id < s.id => -1,
            _ => 1,
        },
    }
}

fn link_conn(app: &AppHandle) -> LinkConn {
    let up = app.try_state::<Arc<Link>>().is_some_and(|l| l.connected().is_some());
    LinkConn { up, side: link_side(app) }
}

fn send_link_conn(app: &AppHandle) {
    let _ = app.emit_to("main", "link-conn", link_conn(app));
}

fn link_view(app: &AppHandle, link: &Link) -> LinkView {
    LinkView { status: link.status(), side: link_side(app) }
}

fn send_link_status(app: &AppHandle) {
    if let Some(link) = app.try_state::<Arc<Link>>() {
        let _ = app.emit_to("status", "link-status", link_view(app, &link));
    }
}

/// link.rs 送出來的事轉給視窗
fn on_link_event(app: &AppHandle, event: link::Event) {
    match event {
        link::Event::Msg(msg) => {
            let _ = app.emit_to("main", "link", msg);
        }
        link::Event::Conn => send_link_conn(app),
        link::Event::Changed => send_link_status(app),
    }
}

fn parse_ips(ips: &[String]) -> Vec<Ipv4Addr> {
    ips.iter().filter_map(|s| s.parse().ok()).collect()
}

/// 在區網上顯示的電腦名稱：Windows 是 DESKTOP-XXXX 這類，macOS 是「小明的 MacBook Air」這類
fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = Command::new("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "毛球貓".into())
}

#[tauri::command]
fn link_status(app: AppHandle, link: State<Arc<Link>>) -> LinkView {
    link_view(&app, &link)
}

/// 貓的視窗剛載入時問一次；之後連上、斷線會收到 link-conn
#[tauri::command]
fn link_state(app: AppHandle) -> LinkConn {
    link_conn(&app)
}

#[tauri::command]
fn link_send(link: State<Arc<Link>>, msg: serde_json::Value) -> Result<(), String> {
    link.send(&msg)
}

#[tauri::command]
fn set_link_on(app: AppHandle, link: State<Arc<Link>>, on: bool) {
    update_settings(&app, |s| s.link.on = on);
    link.set_on(on);
}

/// 勾選要跟哪一台連；id 是 None 就是取消。只能勾清單上找得到的電腦
#[tauri::command]
fn set_link_peer(app: AppHandle, link: State<Arc<Link>>, id: Option<String>) -> Result<(), String> {
    let peer = match id {
        Some(id) => {
            let name = link.found_name(&id).ok_or("找不到這台電腦")?;
            Some(LinkPeer { id, name })
        }
        None => None,
    };
    // 先存設定再換人：link.rs 換人時會通知狀態視窗，那時算「隔壁在哪一邊」要用新的那台
    update_settings(&app, |s| s.link.peer = peer.clone());
    link.set_peer(peer.map(|p| (p.id, p.name)));
    send_link_conn(&app);
    Ok(())
}

/// 隔壁在我的哪一邊；None 是自動
#[tauri::command]
fn set_link_side(app: AppHandle, side: Option<String>) -> Result<(), String> {
    if side.as_deref().is_some_and(|s| s != "left" && s != "right") {
        return Err(format!("沒有這一邊：{side:?}"));
    }
    update_settings(&app, |s| s.link.side = side);
    send_link_conn(&app);
    send_link_status(&app);
    Ok(())
}

/// 手動輸入 IP：不同網段時廣播傳不過去，改成直接送過去
#[tauri::command]
fn add_link_ip(app: AppHandle, link: State<Arc<Link>>, ip: String) -> Result<(), String> {
    let addr: Ipv4Addr = ip.trim().parse().map_err(|_| format!("不是 IP：{ip}"))?;
    if addr.is_unspecified() || addr.is_broadcast() || addr.is_multicast() || addr.is_loopback() {
        return Err(format!("不能用這個 IP：{addr}"));
    }
    let mut ips = Vec::new();
    update_settings(&app, |s| {
        let text = addr.to_string();
        if !s.link.ips.contains(&text) {
            s.link.ips.push(text);
        }
        if s.link.ips.len() > LINK_MAX_IPS {
            s.link.ips.remove(0);
        }
        ips = s.link.ips.clone();
    });
    link.set_ips(parse_ips(&ips));
    Ok(())
}

#[tauri::command]
fn remove_link_ip(app: AppHandle, link: State<Arc<Link>>, ip: String) {
    let mut ips = Vec::new();
    update_settings(&app, |s| {
        s.link.ips.retain(|x| *x != ip);
        ips = s.link.ips.clone();
    });
    link.set_ips(parse_ips(&ips));
}

#[tauri::command]
fn open_picture_folder(app: AppHandle) {
    open_picture_dir(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn hide_status(app: AppHandle) {
    hide_status_window(&app);
}

/* ─── 狀態視窗：左鍵點系統匣圖示打開，看貓在做什麼、換毛色、收放家具、選螢幕 ─── */

/// 狀態視窗的大小（邏輯像素）
const STATUS_SIZE: (f64, f64) = (360.0, 380.0);

/// 離工作列、螢幕邊緣多遠（邏輯像素）
const STATUS_MARGIN: f64 = 8.0;

/// 視窗開著時點系統匣圖示：視窗會先失去焦點收起來，才收到這次點擊。
/// 記下收起來的時間，剛收起來就收到的點擊當作「關掉」，不然會關了又馬上打開
static STATUS_HIDDEN_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// 第一次打開時才建立，之後只是藏起來、再拿出來，不用每次重新載入
fn build_status_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let win = WebviewWindowBuilder::new(app, "status", WebviewUrl::App("status.html".into()))
        .title("毛球貓")
        .inner_size(STATUS_SIZE.0, STATUS_SIZE.1)
        .decorations(false)
        .shadow(true) // Windows 11：沒有標題列的視窗也有陰影和圓角
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        // 建立時不拿焦點。預設會在視窗還藏著時就讓 WebView 拿到焦點，
        // 等 show 把視窗叫出來，這個焦點又被搬走；Windows 的 Focused 事件
        // 是照 WebView 的焦點送的，於是送來一個 Focused(false)，剛打開的
        // 視窗就被下面的「點到別的地方就收起來」收掉。只有第一次打開會這樣：
        // 之後視窗已經建好，不會再經過建立這一步。
        // 焦點交給 show_status_window 的 show、set_focus，跟之後每次打開一樣
        .focused(false)
        .build()?;
    let handle = app.clone();
    win.on_window_event(move |event| match event {
        // 點到別的地方就收起來，跟系統匣的其他面板一樣
        WindowEvent::Focused(false) => hide_status_window(&handle),
        // Alt+F4 也只是收起來
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide_status_window(&handle);
        }
        _ => {}
    });
    Ok(win)
}

fn hide_status_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("status") else {
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        return;
    }
    let _ = win.hide();
    if let Ok(mut at) = STATUS_HIDDEN_AT.lock() {
        *at = Some(Instant::now());
    }
    // 貓的視窗不用再每半秒送狀態過來
    let _ = app.emit_to("main", "status-visible", false);
}

/// 左鍵點系統匣圖示：開著就關、關著就開（rect 是圖示的位置）。
/// 圖示的位置用 tauri::Rect 寫全名：上面長條用的 Rect 是另一個同名的 (x, y, 寬, 高)
fn toggle_status_window(app: &AppHandle, rect: tauri::Rect) {
    let just_hidden = STATUS_HIDDEN_AT
        .lock()
        .ok()
        .and_then(|at| *at)
        .is_some_and(|at| at.elapsed() < Duration::from_millis(400));
    let open = app
        .get_webview_window("status")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if open {
        hide_status_window(app);
    } else if !just_hidden {
        show_status_window(app, Some(rect));
    }
}

/// 打開狀態視窗；從選單打開時沒有圖示位置，擺在游標旁邊
fn show_status_window(app: &AppHandle, anchor: Option<tauri::Rect>) {
    let (win, fresh) = match app.get_webview_window("status") {
        Some(w) => (w, false),
        None => match build_status_window(app) {
            Ok(w) => (w, true),
            Err(_) => return,
        },
    };
    place_status_window(app, &win, anchor);
    let _ = win.show();
    // 剛建好、還沒顯示過的視窗，Windows 還沒算好無邊框陰影的外框，
    // 上面擺的位置和大小會偏掉（第一次打開會太下面）。顯示後再擺一次，跟之後每次打開一樣
    if fresh {
        place_status_window(app, &win, anchor);
    }
    let _ = win.set_focus();
    let _ = app.emit_to("status", "status-shown", ());
    let _ = app.emit_to("main", "status-visible", true);
}

/// 擺在圖示旁邊：水平對準圖示中間、不超出螢幕；
/// 工作列在下面（Windows 預設）就貼著工作列往上長，在上面（macOS 選單列）就往下長
fn place_status_window(app: &AppHandle, win: &WebviewWindow, anchor: Option<tauri::Rect>) {
    // 圖示的位置在 Windows 是實體像素；萬一給的是邏輯像素，先用主螢幕的縮放比例換算
    let scale_guess = app.primary_monitor().ok().flatten().map_or(1.0, |m| m.scale_factor());
    let (cx, cy) = match anchor {
        Some(r) => {
            let p = r.position.to_physical::<f64>(scale_guess);
            let s = r.size.to_physical::<f64>(scale_guess);
            (p.x + s.width / 2.0, p.y + s.height / 2.0)
        }
        None => match app.cursor_position() {
            Ok(p) => (p.x, p.y),
            Err(_) => return,
        },
    };
    let Ok(Some(monitor)) = app.monitor_from_point(cx, cy) else {
        return;
    };
    let scale = monitor.scale_factor();
    let (w, h) = (STATUS_SIZE.0 * scale, STATUS_SIZE.1 * scale);
    let margin = STATUS_MARGIN * scale;
    let area = monitor.work_area();
    let left = area.position.x as f64;
    let top = area.position.y as f64;
    let right = left + area.size.width as f64;
    let bottom = top + area.size.height as f64;
    let x = (cx - w / 2.0).min(right - w - margin).max(left + margin);
    let screen_mid = monitor.position().y as f64 + monitor.size().height as f64 / 2.0;
    let y = if cy > screen_mid { bottom - h - margin } else { top + margin };
    // 先搬位置再設大小：搬到縮放比例不同的螢幕時，Windows 會先依新比例縮放視窗（同 apply_rect）
    let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    let _ = win.set_size(LogicalSize::new(STATUS_SIZE.0, STATUS_SIZE.1));
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            set_passthrough,
            list_pictures,
            read_picture,
            get_settings,
            set_coat,
            set_furniture,
            list_screens,
            set_screen,
            diary_add,
            set_diary,
            open_picture_folder,
            quit_app,
            hide_status,
            link_status,
            link_state,
            link_send,
            set_link_on,
            set_link_peer,
            set_link_side,
            add_link_ip,
            remove_link_ip
        ])
        .setup(|app| {
            // macOS：不在 Dock 出現，只留選單列的貓咪圖示
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 設定要在擺長條之前讀：貓待在哪個螢幕也是設定
            let mut settings = load_settings(app.handle());
            if let Ok(mut chosen) = CHOSEN_SCREEN.lock() {
                *chosen = settings.screen.clone();
            }
            // 第一次開：產生這台電腦的代號（下次改設定時一起存起來）
            if !link::valid_id(&settings.link.id) {
                settings.link.id = link::new_id();
            }
            let link_settings = settings.link.clone();
            app.manage(Mutex::new(settings));

            // 串門子：「連線」打開過的話，接著找人、連上次勾的那台
            let handle = app.handle().clone();
            let link = Link::new(link_settings.id, computer_name(), (link::PORT, link::PORT), move |e| {
                on_link_event(&handle, e)
            });
            app.manage(link.clone());
            link.set_ips(parse_ips(&link_settings.ips));
            link.set_peer(link_settings.peer.map(|p| (p.id, p.name)));
            link.set_on(link_settings.on);

            let win = app
                .get_webview_window("main")
                .expect("找不到 main 視窗");
            if let Some(r) = strip_rect(&win)? {
                apply_rect(&win, r)?;
            }
            win.set_ignore_cursor_events(true)?;
            win.show()?;
            watch_cursor(win.clone());

            // 系統匣：左鍵打開狀態視窗（毛色、家具、螢幕、泡泡圖片資料夾都在那裡，選好會記住）；
            // 右鍵選單只放當下要用的：換玩具、餵貓條、安靜模式（開會、分享螢幕時讓貓去睡），這些不記
            let toy_ball = CheckMenuItem::with_id(app, "toy:ball", "毛球", true, true, None::<&str>)?;
            let toy_laser = CheckMenuItem::with_id(app, "toy:laser", "雷射光", true, false, None::<&str>)?;
            let toy_wand = CheckMenuItem::with_id(app, "toy:wand", "逗貓棒", true, false, None::<&str>)?;
            let toys = Submenu::with_items(app, "玩具", true, &[&toy_ball, &toy_laser, &toy_wand])?;
            // 餵貓條：在貓前面掉一條下來（地上已經有一條就不會再掉）
            let treat = MenuItem::with_id(app, "treat", "餵貓條", true, None::<&str>)?;
            let quiet = CheckMenuItem::with_id(app, "quiet", "安靜模式", true, false, None::<&str>)?;
            let status = MenuItem::with_id(app, "status", "狀態與設定…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束毛球貓", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&toys, &treat, &quiet, &sep1, &status, &sep2, &quit])?;
            let quiet_item = quiet.clone();
            let toy_items = [toy_ball, toy_laser, toy_wand];
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().expect("缺少圖示").clone())
                .tooltip("毛球貓")
                .menu(&menu)
                .show_menu_on_left_click(false) // 左鍵是狀態視窗，右鍵才是選單
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_status_window(tray.app_handle(), rect);
                    }
                })
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quiet" => {
                        let on = quiet_item.is_checked().unwrap_or(false);
                        let _ = app.emit("quiet", on);
                    }
                    "treat" => {
                        let _ = app.emit("treat", ());
                    }
                    "status" => show_status_window(app, None),
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
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("毛球貓啟動失敗");
}
