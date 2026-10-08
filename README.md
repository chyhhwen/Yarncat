# 毛球貓 YarnCat

一隻住在螢幕底部的原創像素貓。你辦公，牠在旁邊玩毛球：盯著球、壓低身體扭屁股、飛撲、用前掌撥，玩累了就趴下睡覺。也可以換成雷射光或逗貓棒，用滑鼠陪牠玩。

支援 Windows 10 / 11。macOS 版（M 系列、Intel 都可以）是測試版，作者沒有 Mac 實際測過，有問題請到 [Issues](../../issues) 回報。

## 安裝

### Windows

1. 到 [Releases](../../releases) 下載最新的 `YarnCat_x.x.x_x64-setup.exe`。
2. 雙擊安裝。
3. 因為安裝檔沒有程式碼簽章，Windows 可能會跳出「Windows 已保護您的電腦」。按「其他資訊」→「仍要執行」即可。

### macOS（測試版）

1. 到 [Releases](../../releases) 下載最新的 `YarnCat_x.x.x_universal.dmg`。
2. 打開 dmg，把 YarnCat 拖進「應用程式」。
3. 因為安裝檔沒有簽章，直接打開會說「YarnCat 已損毀」。先打開「終端機」執行一次：
   ```
   xattr -cr /Applications/YarnCat.app
   ```
   之後就能正常打開。

## 怎麼玩

- 貓和毛球只在工作列上方一條窄區活動，透明的地方滑鼠會直接點到後面的程式。
- 拖住貓可以把牠拎起來，放開會掉下去。
- 抓住毛球甩出去，貓會追。
- 系統匣的貓咪圖示（Windows 在右下角，macOS 在右上角選單列）：
  - **玩具**：換成別的玩具。
    - **毛球**：預設的玩具。
    - **雷射光**：紅點照在滑鼠游標的位置。貓會追過去，跳起來拍，但永遠抓不到。
    - **逗貓棒**：逗貓棒跟著滑鼠走，羽毛垂在游標下方。貓會追著跑，跳起來抓住羽毛吊在上面；甩太快牠就會鬆手。
  - **安靜模式**：開會或分享螢幕時讓貓去睡覺。
  - **結束毛球貓**。

## 原理

| 部分 | 檔案 | 做什麼 |
| --- | --- | --- |
| 透明長條視窗 | `src-tauri/tauri.conf.json` | 透明、無邊框、置頂、不出現在工作列 |
| 擺放位置 | `src-tauri/src/main.rs` `place_strip` | 依螢幕扣掉工作列的範圍，擺在最底部 |
| 點擊穿透 | `main.rs` `watch_cursor` + `src/index.html` `hitTest` | Rust 每 16ms 回報游標位置，前端判斷在不在貓或毛球上，再開關穿透 |
| 貓的行為 | `src/index.html` `updateCat` | 狀態機：坐、舔毛、盯、追、撲、撥、睡；雷射光、逗貓棒另外有追、壓低、跳起來拍、抓住吊著 |
| 毛球 | `src/index.html` `updateBall` | 重力、摩擦、反彈，線頭用 Verlet 積分 |
| 雷射光、逗貓棒 | `src/index.html` `updateLaser`、`updateWand` | 紅點、逗貓棒的線都掛在游標上（`watch_cursor` 回報的位置） |
| 系統匣選單 | `src-tauri/src/main.rs` `main` | 換玩具、安靜模式、結束，用事件通知前端 |

直接用瀏覽器打開 `src/index.html` 也能玩（沒有點擊穿透），改行為時這樣測最快。按 `1` `2` `3` 切換毛球、雷射光、逗貓棒。

## 自己編譯

需要 [Rust](https://www.rust-lang.org/tools/install) 和 [Node.js](https://nodejs.org/)：

```
npm install
npm run tauri dev
```
