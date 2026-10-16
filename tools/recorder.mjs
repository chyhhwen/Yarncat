// 錄影：在無頭瀏覽器裡打開 src/index.html，一格一格跑遊戲、截圖，交給 make-gifs.mjs 編成 GIF
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PAGE = pathToFileURL(path.join(ROOT, 'src', 'index.html')).href;

/* 注入頁面的工具 R（劇本就是在頁面裡呼叫這些，加上 index.html 自己的函式和變數）：
   - R.seed：亂數固定種子，同一個劇本每次錄出來都一樣
   - R.tick：只跑遊戲不截圖，擺場景用。更新的順序照 index.html 的 frame()，那邊改了這裡也要跟著改
   - R.step：跑遊戲、每 3 格（20fps）截一張；performance.now 照這裡的模擬時間走，點一下、甩的速度、摸的節奏才對
   - R.move、R.glide、R.down、R.up：用合成的滑鼠事件操作，跟真的滑鼠走同一套程式；截圖時把像素游標畫在 R.cur
   - R.crop：只留畫面的哪一段；R.calm：不要突然冒泡泡、上廁所、躲紙箱、去貓跳台、撥小毛球、去隔壁 */
const KIT = `(() => {
  let s = 0, now = 1e6;
  const R = window.R = { ticks: 0, frames: [], cur: null, crop: { x0: 0, w: W }, log: [] };
  R.seed = n => { s = n | 0; };
  Math.random = () => { s = (s + 0x6D2B79F5) | 0; let t = Math.imul(s ^ (s >>> 15), 1 | s); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };   // mulberry32
  window.requestAnimationFrame = () => 0;              // 停掉遊戲迴圈
  performance.now = () => now;
  R.calm = () => { wishCD = pooCD = boxCD = treeCD = pomCD = visitCD = 1e9; };
  R.tick = (n = 1) => { for (let i = 0; i < n; i++) {
    if (toy === 'ball') updateBall(); else if (toy === 'laser') updateLaser(); else updateWand();
    updateCat(); updateGuest(); linkTick(); updateFurniture(); updateFx(); } };
  const CURSOR = ['O......', 'OO.....', 'OWO....', 'OWWO...', 'OWWWO..', 'OWWWWO.', 'OWWWWWO', 'OWWWOOO', 'OWOWO..', 'OO.OWO.', '....OO.'];
  const cap = document.createElement('canvas'), o = cap.getContext('2d');
  const fire = (type, x, y) => window.dispatchEvent(new PointerEvent(type, { clientX: x * S, clientY: y * S, pointerId: 1, bubbles: true, button: 0, buttons: type === 'pointerup' ? 0 : 1 }));
  R.move = (x, y) => { R.cur = { x, y }; fire('pointermove', x, y); };
  R.hide = () => { R.cur = null; };
  R.down = () => fire('pointerdown', R.cur.x, R.cur.y);
  R.up = () => fire('pointerup', R.cur.x, R.cur.y);
  R.capture = () => {
    frame(lastT);                                      // 只畫不更新
    cap.width = W; cap.height = H;
    o.fillStyle = '#4b5b72'; o.fillRect(0, 0, W, H);   // 網頁版的底色
    o.drawImage(cv, quake ? (quake % 4 < 2 ? 1 : -1) : 0, 0);   // 貓蹦出來嚇人時整條畫面震一下（shakeView）
    if (R.cur) { const cx = Math.round(R.cur.x), cy = Math.round(R.cur.y);
      CURSOR.forEach((row, j) => [...row].forEach((ch, i) => { if (ch === '.') return; o.fillStyle = ch === 'O' ? C.O : '#ffffff'; o.fillRect(cx + i, cy + j, 1, 1); })); }
    const d = o.getImageData(Math.round(R.crop.x0), 0, R.crop.w, H).data, rgb = new Uint8Array(R.crop.w * H * 3);
    for (let i = 0, j = 0; i < d.length; i += 4, j += 3) { rgb[j] = d[i]; rgb[j + 1] = d[i + 1]; rgb[j + 2] = d[i + 2]; }
    R.frames.push(rgb);
  };
  R.step = (n = 1, each) => { for (let i = 0; i < n; i++) {
    if (each) each(i);
    R.tick(); now += 1000 / 60; R.ticks++;
    if (R.log.length === 0 || R.log[R.log.length - 1].s !== cat.state) R.log.push({ t: R.ticks, s: cat.state });
    if (R.ticks % 3 === 0) R.capture(); } };
  R.until = (cond, max = 3000) => { for (let i = 0; i < max && !cond(); i++) R.step(); return cond(); };
  R.glide = (x1, y1, n, ease = true) => {             // 游標在 n 格內滑到 (x1, y1)，邊滑邊跑遊戲
    const x0 = R.cur.x, y0 = R.cur.y;
    R.step(n, i => { const k = (i + 1) / n, e = ease ? k * k * (3 - 2 * k) : k; R.move(x0 + (x1 - x0) * e, y0 + (y1 - y0) * e); });
  };
  R.chunk = (i, n) => R.frames.slice(i, i + n).map(f => { let s = ''; for (let j = 0; j < f.length; j += 8192) s += String.fromCharCode(...f.subarray(j, j + 8192)); return btoa(s); });
})()`;

async function framesOf(page) {                        // 把截好的畫面拿回來（每張是寬 × 高 × 3 的 RGB）
  const [n, w, h] = await page.eval('[R.frames.length, R.crop.w, H]');
  const frames = [];
  for (let i = 0; i < n; i += 40) for (const s of await page.eval(`R.chunk(${i}, 40)`)) frames.push(Buffer.from(s, 'base64'));
  return { frames, w, h };
}

export async function recordSingle(browser, story, seed = story.seed) {   // 一個視窗的劇本：場景、劇情都在頁面裡跑完
  const page = await browser.newPage(story.width, 150);
  await page.goto(PAGE);
  await page.eval(KIT);
  await page.eval(`R.seed(${seed})`);
  await page.eval(story.setup);
  await page.eval(story.script);
  return { ...await framesOf(page), errors: page.errors };
}

/* 兩個並排視窗的劇本（串門子）：兩個分頁互當隔壁，輪流各跑幾格，最後左右拼成一張，中間一條深色分隔線。
   哪個分頁在左邊每次不一定（看兩邊隨機的代號），所以找到對方之後才照左右給種子 */
export async function recordPair(browser, story, seed = story.seed) {
  const a = await browser.newPage(story.width, 150), b = await browser.newPage(story.width, 150);
  await a.goto(PAGE); await b.goto(PAGE);
  await a.eval(KIT); await b.eval(KIT);
  for (let i = 0; i < 80; i++) { await a.eval('R.tick(2)'); await b.eval('R.tick(2)'); }   // 先互相找到對方
  if (!(await a.eval('!!peer')) || !(await b.eval('!!peer'))) throw new Error('兩個分頁沒有找到對方');
  const left = (await a.eval('peerSide()')) > 0 ? a : b, right = left === a ? b : a;
  await left.eval(`R.seed(${seed})`); await right.eval(`R.seed(${seed + 1})`);
  await left.eval(story.left); await right.eval(story.right);
  await left.eval(story.start.left); await right.eval(story.start.right);
  const round = async () => { await left.eval(story.step.left); await right.eval(story.step.right); };
  for (let i = 0; i < 600; i++) { await round(); if (await left.eval(story.done)) break; }
  for (let i = 0; i < story.tail; i++) await round();
  const L = await framesOf(left), Rt = await framesOf(right);
  const { w, h } = L, GAP = 2, CW = w * 2 + GAP, n = Math.min(L.frames.length, Rt.frames.length), frames = [];
  for (let i = 0; i < n; i++) {
    const rgb = Buffer.alloc(CW * h * 3);
    for (let y = 0; y < h; y++) {
      L.frames[i].copy(rgb, y * CW * 3, y * w * 3, (y * w + w) * 3);
      for (let x = 0; x < GAP; x++) rgb.set([0x15, 0x15, 0x1b], (y * CW + w + x) * 3);
      Rt.frames[i].copy(rgb, (y * CW + w + GAP) * 3, y * w * 3, (y * w + w) * 3);
    }
    frames.push(rgb);
  }
  return { frames, w: CW, h, errors: [...a.errors, ...b.errors] };
}
