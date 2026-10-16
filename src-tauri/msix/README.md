# 商店版（MSIX）

把 `tauri build` 編好的 exe 包成 MSIX，上架 Microsoft Store。商店審核通過後會幫 MSIX 簽章，從商店安裝的人不會被 SmartScreen、智慧型應用程式控制擋下來。

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

- `YarnCat_x.x.x.0_x64.msix`：上傳到 Partner Center
- `test` 資料夾：在自己電腦測試用

還不能送審的原因（Partner Center 的值還沒填、版本號是 0 開頭）會用黃色警告顯示在那次執行的頁面最上面。

電腦裝了 Rust 和 Windows SDK 的話，也可以自己包，產出在 `src-tauri\target\msix`：

```
npm run tauri build -- --no-bundle
powershell -ExecutionPolicy Bypass -File src-tauri\msix\build.ps1
```

## 在自己電腦測試

1. 「設定」→「系統」→「開發人員專用」，打開「開發人員模式」。
2. 把下載的 `msix.zip` 解壓縮，在 `test` 資料夾裡按右鍵「在終端機中開啟」，執行：
   ```
   Add-AppxPackage -Register "$PWD\AppxManifest.xml"
   ```
3. 從開始功能表打開毛球貓。它直接執行 `test` 資料夾裡的檔案，測完之前不要刪掉資料夾。

移除：「設定」→「應用程式」→「已安裝的應用程式」→ 毛球貓 →「解除安裝」。要從商店安裝之前也要先移除，兩個是同一個 app。

測試版沒有簽章，開著「智慧型應用程式控制」的電腦可能會擋下來；商店版有簽章，不會。

## 送審

上傳 .msix 之後，Partner Center 會多問幾件事，下面的文字可以直接貼。

**runFullTrust 的理由**（欄位很短，太長會被截掉）：

```
Win32 desktop app (Tauri + WebView2) packaged as MSIX. Needs full trust for its transparent always-on-top window and tray icon.
```

**Notes for certification**（毛球貓沒有視窗，要告訴審核人員去哪裡找）：

```
YarnCat has no regular window or taskbar button. After launch, a pixel cat appears at the bottom of the primary screen, just above the taskbar. All options (toys, furniture, quiet mode, quit) are in the cat icon in the system tray; on Windows 11 it may be hidden under the ^ overflow. Drag the cat or the yarn ball with the mouse to play. The app makes no network connections.
```

商店頁另外要準備：

- 隱私權政策網址：可以連到 README 的「隱私」一節，但「安裝程式會下載 WebView2」那條不適用商店版
- 至少一張 1366×768 以上的 PNG 截圖：`docs` 裡的 GIF 是網頁版錄的，要另外截桌寵版在桌面上的樣子
- 年齡分級問卷、介紹文字

## 注意

- 商店版不會在安裝時裝 WebView2。Windows 11 內建，絕大多數 Windows 10 也已經有了。
- 泡泡圖片資料夾（`圖片\YarnCat`）照樣讀得到。寫進 AppData 的東西（WebView2 的快取）會被 Windows 導到 app 自己的資料夾，解除安裝時一起清掉。
- `icons/icon.png` 是 16×16 的像素畫放大 32 倍。`build.ps1` 先還原成 16×16，再放大整數倍，邊緣才不會糊。換成不是像素畫的圖示，`build.ps1` 會報錯，到時候要改它的縮放方式。
- `resources.pri` 是圖示的索引，Windows 靠它照螢幕縮放比例挑尺寸。`makepri` 預設會把不同縮放比例的圖示拆到另外的檔案，只包一個 .msix 時會找不到，所以 `build.ps1` 把拆檔關掉了。
