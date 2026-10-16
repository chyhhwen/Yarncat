// 開一個無頭的 Edge／Chrome，用 Chrome DevTools Protocol 操作分頁。只用 Node 內建的 fetch、WebSocket，不用裝套件
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CANDIDATES = {
  win32: [
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
    'C:/Program Files/Google/Chrome/Application/chrome.exe',
    path.join(process.env.LOCALAPPDATA || '', 'Google/Chrome/Application/chrome.exe'),
  ],
  darwin: [
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
  ],
  linux: ['/usr/bin/microsoft-edge', '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'],
};
const sleep = ms => new Promise(r => setTimeout(r, ms));

function findBrowser() {
  const found = process.env.BROWSER || (CANDIDATES[process.platform] || []).find(p => fs.existsSync(p));
  if (!found) throw new Error('找不到 Edge 或 Chrome：請用環境變數 BROWSER 指定瀏覽器執行檔的路徑');
  return found;
}

/* 啟動瀏覽器。回傳 { newPage(寬, 高), close() }；同一個瀏覽器裡的分頁可以用 BroadcastChannel 互相傳訊息（串門子要用）。
   設定檔放在系統的暫存資料夾，關掉就刪；除錯用的 port 讓瀏覽器自己挑，不會跟別的程式撞到 */
export async function launch() {
  if (!globalThis.WebSocket) throw new Error('需要 Node.js 22 以上（要用內建的 WebSocket）');
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'yarncat-gif-'));
  const proc = spawn(findBrowser(), ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--hide-scrollbars', '--mute-audio', 'about:blank'], { stdio: 'ignore' });
  let port = 0;                                        // 瀏覽器挑好的 port 寫在設定檔資料夾的 DevToolsActivePort 第一行
  for (let i = 0; i < 150 && !port; i++) {
    try { port = Number(fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]) || 0; } catch {}
    if (!port) await sleep(100);
  }
  if (!port) { proc.kill(); throw new Error('瀏覽器沒有啟動'); }
  const base = `http://127.0.0.1:${port}`, pages = [];
  return {
    async newPage(width, height) {                     // 第一個分頁用瀏覽器開好的那個，之後再開新的
      const opened = pages.length ? null : (await (await fetch(`${base}/json/list`)).json()).find(t => t.type === 'page');
      const target = opened || await (await fetch(`${base}/json/new?about:blank`, { method: 'PUT' })).json();
      const page = await attach(target, width, height);
      pages.push(page);
      return page;
    },
    close() {
      for (const p of pages) p.ws.close();
      try {
        if (process.platform === 'win32') execFileSync('taskkill', ['/PID', String(proc.pid), '/T', '/F'], { stdio: 'ignore' });
        else proc.kill('SIGKILL');
      } catch {}
      try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); } catch {}
    },
  };
}

async function attach(target, width, height) {        // 連上一個分頁：goto 打開網址、eval 在頁面裡執行一段程式並拿回結果
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let id = 0, onLoad = null;
  const pending = new Map(), errors = [];
  ws.onmessage = ev => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result);
    } else if (m.method === 'Page.loadEventFired' && onLoad) onLoad();
    else if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  return {
    ws, errors,
    async goto(url) {
      const loaded = new Promise(r => { onLoad = r; });
      await send('Page.navigate', { url });
      await loaded;
    },
    async eval(expression) {
      const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      return r.result.value;
    },
  };
}
