# tools

## 重錄 README 的 GIF

```
node tools/make-gifs.mjs                              # 錄 hero、box、tree，蓋掉 docs/ 裡的
node tools/make-gifs.mjs tree                         # 只錄一支
node tools/make-gifs.mjs visit                        # 串門子：兩個視窗並排，存成 docs/visit.gif
node tools/make-gifs.mjs hero --seed 7 --out tmp      # 換亂數種子，先存到 tmp/ 看看
```

需要：

- Node.js 22 以上：用內建的 fetch、WebSocket，不用 `npm install`
- Edge 或 Chrome：Windows、macOS 的預設安裝位置會自己找；找不到就用環境變數 `BROWSER` 指定執行檔的路徑

### 怎麼錄的

用無頭瀏覽器打開 `src/index.html`，停掉遊戲迴圈，改成一格一格跑。每 3 格（20fps）截一張，畫上像素游標，裁好之後放大 3 倍編成 GIF，最後一格停 1 秒。亂數用固定的種子，同一個劇本每次錄出來都一模一樣。

游標用合成的滑鼠事件操作（點一下、拖、甩、摸），跟真的滑鼠走同一套程式。

### 檔案

| 檔案 | 做什麼 |
| --- | --- |
| `make-gifs.mjs` | 指令：錄哪幾支、存到哪 |
| `stories.mjs` | 每支 GIF 的劇本：場景怎麼擺、游標怎麼動、什麼時候收尾 |
| `recorder.mjs` | 注入頁面的工具 `R`（一格一格跑、截圖、畫游標），以及一個視窗、兩個並排視窗的錄法 |
| `browser.mjs` | 開無頭的 Edge／Chrome，用 DevTools Protocol 操作分頁 |
| `gifenc.mjs` | GIF 編碼器：每格只存有變的範圍，相同的格子合併 |

### 注意

- 劇本直接呼叫 `index.html` 裡的函式和變數（`setState`、`cat`、`guest`……）；`R.tick` 也照 `frame()` 的更新順序跑。那邊改了名字或順序，這裡要跟著改。
- 改了貓的行為，同一個種子錄出來可能就不一樣了。hero 最明顯：貓怎麼追毛球全看亂數。可以用 `--seed` 換幾個種子試試，挑一段好看的，再把 `stories.mjs` 裡的 `seed` 改掉。
- 錄完先打開 GIF 看過再 commit。
