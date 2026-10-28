# 商店版（MSIX）

把 `tauri build` 編好的 exe 包成 MSIX，上架 Microsoft Store。商店審核通過後會幫 MSIX 簽章，從商店安裝的人不會被 SmartScreen、智慧型應用程式控制擋下來。

已上架：[Microsoft Store 的毛球貓頁面](https://apps.microsoft.com/detail/9nrj7lgz2jkd?hl=zh-TW&gl=TW)

| 檔案 | 做什麼 |
| --- | --- |
| `AppxManifest.xml` | 套件資訊：名稱、發行者、權限、圖示。版本號和 exe 檔名由 `build.ps1` 填 |
| `build.ps1` | 從 `icons/icon.png` 產生各種尺寸的圖示，建 `resources.pri`，用 `makeappx` 打包 |

## Partner Center 給的值

`AppxManifest.xml` 裡這三個值抄自 Partner Center「Product management」→「Product identity」頁。這是商店指定的，大小寫、空格都要一樣，不能改：

| Product identity 頁 | AppxManifest.xml |
| --- | --- |
| `Package/Identity/Name` | `<Identity Name="…">` |
| `Package/Identity/Publisher` | `<Identity Publisher="CN=…">` |
| `Package/Properties/PublisherDisplayName` | `<PublisherDisplayName>` |

兩個 `DisplayName`（`<DisplayName>` 和 `uap:VisualElements` 的）是開始功能表和商店顯示的名字，必須是 Partner Center 保留過的名稱，不然上傳時會被拒（This package's manifest uses a display name that you have not reserved）。現在用的是「毛球貓」，要在「Product management」→「Manage app names」保留過。改名字的話，先在那裡保留新名字，再改這兩個地方。

這三個值是 `FILL-IN` 的時候也能打包、在自己電腦測，只是那個 .msix 不能送審，打包時會出現警告。

## 版本號

自動用 `tauri.conf.json` 的版本，後面補一個 `.0`（1.0.0 → 1.0.0.0）。商店的規定：

- 第一段不能是 0：0.x 版不能送審，要先把 `package.json`、`tauri.conf.json`、`Cargo.toml` 升到 1.0.0 以上
- 第四段要是 0，留給商店用
- 版本號要比上一次送審的高，已經安裝的人才會收到更新

## 產生 .msix

推 `v` 或 `V` 開頭的標籤（跟平常發版一樣）。GitHub Actions 包好 Windows 安裝檔之後會接著包 MSIX，放在那次執行的頁面最下面「Artifacts」的 `msix`：

- `YarnCat_x.x.x.0_x64.msix`：上傳到 Partner Center。沒有簽章，點兩下裝不起來（見下一節）
- `test` 資料夾：在自己電腦測試用

還不能送審的原因（Partner Center 的值還沒填、版本號是 0 開頭）會用黃色警告顯示在那次執行的頁面最上面。

電腦裝了 Rust 和 Windows SDK 的話，也可以自己包，產出在 `src-tauri\target\msix`：

```
npm run tauri build -- --no-bundle
powershell -ExecutionPolicy Bypass -File src-tauri\msix\build.ps1
```

## 在自己電腦測試

不要直接點兩下 .msix：它沒有簽章，Windows 會說「無法驗證此應用程式套件的發行者憑證」（0x800B010A）。這是正常的，商店審核通過後會幫它簽章。要在自己電腦測，用 `test` 資料夾：

1. 按 Win + R，輸入 `ms-settings:developers`，打開「開發人員模式」（要系統管理員權限）。
2. 裝了 GitHub 版的毛球貓的話，先從系統匣「結束毛球貓」，不然畫面上會有兩隻貓。
3. 把下載的 `msix.zip` 解壓縮，在 `test` 資料夾裡按右鍵「在終端機中開啟」，執行：
   ```
   Add-AppxPackage -Register "$PWD\AppxManifest.xml"
   ```
   出現 0x80073CFF 表示開發人員模式還沒打開。
4. 從開始功能表打開毛球貓。它直接執行 `test` 資料夾裡的檔案，測完之前不要刪掉資料夾。

移除：「設定」→「應用程式」→「已安裝的應用程式」→ 毛球貓 →「解除安裝」。要從商店安裝之前也要先移除，兩個是同一個 app。

測試版沒有簽章，開著「智慧型應用程式控制」的電腦可能會擋下來；商店版有簽章，不會。

## 送審

上傳 .msix 之後，Partner Center 會多問幾件事，下面的文字可以直接貼。

**runFullTrust 的理由**（Submission options 頁。有人遇過這欄超過字數會被截掉，所以重點放在最前面）：

```
Win32 desktop pet (Tauri + WebView2) packaged as MSIX, not UWP. Full trust is needed for its transparent, always-on-top, click-through window above the taskbar, its tray menu, reading the cursor position so the cat can chase toys, and reading images from (and writing a small local diary file to) Pictures\YarnCat.
```

**Notes for certification**（貼在「Additional Testing Information」頁。毛球貓沒有一般的視窗，要告訴審核人員去哪裡找）：

```
YarnCat has no regular window or taskbar button. After launch, a pixel cat appears at the bottom of the primary screen, just above the taskbar. Left-click the cat icon in the system tray to open a small status window (status, coat, furniture, screen, link, system); right-click it for toys, treats, quiet mode and quit. On Windows 11 the icon may be hidden under the ^ overflow. Drag the cat or the yarn ball with the mouse to play. Networking is off by default: only if the user turns on the "連線" (Link) tab does the app use the local network (UDP/TCP port 52525) to find another YarnCat on the same LAN and exchange the cats' state with the computer the user paired with. It never contacts internet servers. Optional add-on files (.yarncat, small JavaScript files that add cat coats and animations) are loaded only if the user puts them in Pictures\YarnCat\dlc; none are bundled with or downloaded by the app.
```

**隱私權政策**（Partner Center 規定一定要有，這次選「Provide privacy policy text」直接貼文字。這份只存在 Partner Center，要改的時候這裡也一起改。1.2.0 加了串門子，送 1.2.0 審核時要把 Partner Center 上的舊版換成下面這份；之後加了貓咪日記，下一版送審時也要換）：

```
毛球貓 YarnCat 隱私權政策

毛球貓不會連上網際網路，也不會把任何個人資料傳給開發者或其他人。

・程式裡沒有統計、廣告或自動更新功能。
・串門子的「連線」預設關閉。打開之後，毛球貓才會在區網上廣播這台的電腦名稱，讓同一個區網裡的毛球貓找得到；並跟你勾選的那台電腦直接互傳貓的狀態（毛色、安靜模式、貓在誰家、體力），不經過任何伺服器，不傳游標位置和圖片。關掉就停止。
・設定（毛色、家具、螢幕、連線設定，包括這台電腦的隨機代號）只存在你的電腦上。
・只在你的電腦上記錄貓咪日記，不會傳到任何地方，可以在狀態視窗的「系統」分頁關掉。日記放在「圖片\YarnCat\diary.json」，只記每天的次數和時間（例如玩了多久玩具、摸了幾次），留最近 30 天。
・游標位置只用來讓貓追玩具、判斷你有沒有點到貓，不會記錄下來。
・「圖片\YarnCat」資料夾裡的圖片只在你的電腦上讀取、畫在泡泡裡，不會傳到任何地方。
・畫面由 Windows 的 Microsoft Edge WebView2 元件顯示。這個元件，以及從 Microsoft Store 下載、更新毛球貓的過程，由微軟依照微軟隱私權聲明處理。

有問題請到 https://github.com/chyhhwen/Yarncat/issues 回報。
最後更新：2026 年 10 月 23 日

YarnCat Privacy Policy

YarnCat does not connect to the internet and does not send any personal information to the developer or anyone else.

- The app has no analytics, ads, or automatic updates of its own.
- The "Link" feature (cats visiting each other) is off by default. Only after you turn it on does YarnCat broadcast this computer's name on your local network so other copies of YarnCat on the same network can find it, and exchange the cats' state (coat, quiet mode, which home the cat is in, energy) directly with the computer you paired with. No server is involved, and the cursor position and images are never sent. Turning it off stops it.
- Settings (coat, furniture, screen, and link settings, including a random ID for this computer) are stored only on your computer.
- The cat diary is recorded only on your computer and never sent anywhere. You can turn it off in the status window's "系統" (System) tab. It is stored in Pictures\YarnCat\diary.json and contains only daily counts and durations (for example, how long the cat played with each toy and how many times it was petted), kept for the last 30 days.
- The cursor position is used only to let the cat chase toys and to detect clicks on the cat. It is never recorded.
- Images in the Pictures\YarnCat folder are read only on your computer to show them in the cat's bubbles. They are never sent anywhere.
- The app is displayed with Microsoft Edge WebView2, a Windows component. That component, and downloading and updating YarnCat through the Microsoft Store, are handled by Microsoft under the Microsoft Privacy Statement.

Questions: https://github.com/chyhhwen/Yarncat/issues
Last updated: October 23, 2026
```

商店頁另外要準備：

- 至少一張 1366×768 以上的 PNG 截圖：`docs` 裡的 GIF 是網頁版錄的，要另外截桌寵版在桌面上的樣子
- Store logos 的「Store display images」：300×300、150×150、71×71 三格分別放 `store/app-tile-300.png`、`app-tile-150.png`、`app-tile-71.png`，一樣是像素畫整數倍放大。不給的話商店會拿套件裡的圖示放大。上面的 9:16 Poster art、1:1 Box art 對 app 是選填，要放的話用 `store/poster-1440x2160.png`、`store/box-2160.png`（貓放在上面三分之二，下面留給商店疊字）
- 年齡分級問卷、介紹文字

## 注意

- 商店版不會在安裝時裝 WebView2。Windows 11 內建，絕大多數 Windows 10 也已經有了。
- 泡泡圖片資料夾（`圖片\YarnCat`）、DLC 資料夾（`圖片\YarnCat\dlc`）照樣讀得到。寫進 AppData 的東西（WebView2 的快取）會被 Windows 導到 app 自己的資料夾，解除安裝時一起清掉。
- `icons/icon.png` 是 16×16 的像素畫放大 32 倍。`build.ps1` 先還原成 16×16，再放大整數倍，邊緣才不會糊。換成不是像素畫的圖示，`build.ps1` 會報錯，到時候要改它的縮放方式。
- `resources.pri` 是圖示的索引，Windows 靠它照螢幕縮放比例挑尺寸。`makepri` 預設會把不同縮放比例的圖示拆到另外的檔案，只包一個 .msix 時會找不到，所以 `build.ps1` 把拆檔關掉了。
