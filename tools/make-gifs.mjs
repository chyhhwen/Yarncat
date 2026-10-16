// 重錄 README 的 GIF：node tools/make-gifs.mjs [hero] [box] [tree] [visit] [--seed 數字] [--out 資料夾]
// 沒指定就錄 hero、box、tree，存到 docs/（--out 可以改存別的地方，先看看再決定要不要蓋掉）
import fs from 'node:fs';
import path from 'node:path';
import { launch } from './browser.mjs';
import { encodeGif } from './gifenc.mjs';
import { ROOT, recordPair, recordSingle } from './recorder.mjs';
import * as stories from './stories.mjs';

const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); return i < 0 ? undefined : args.splice(i, 2)[1]; };
const seed = option('--seed'), out = path.resolve(option('--out') || path.join(ROOT, 'docs'));
const names = args.length ? args : ['hero', 'box', 'tree'];

for (const name of names) {
  const story = stories[name];
  if (!story) { console.error(`沒有「${name}」這支，可以錄：${Object.keys(stories).join('、')}`); process.exitCode = 1; continue; }
  const browser = await launch();                     // 每支開一個新的瀏覽器：同一個瀏覽器的分頁會互當隔壁
  try {
    const record = story.pair ? recordPair : recordSingle;
    const { frames, w, h, errors } = await record(browser, story, seed === undefined ? undefined : Number(seed));
    if (errors.length) throw new Error(`頁面出錯：${errors.join('；')}`);
    const timed = frames.map(rgb => ({ rgb, ms: 50 }));   // 每 3 格截一張 = 每張 50ms
    timed[timed.length - 1].ms = 1000;                 // 最後一格停一秒再重播
    const gif = encodeGif(timed, w, h, 3);             // 一格放大成 3×3 個像素
    fs.mkdirSync(out, { recursive: true });
    const file = path.join(out, `${name}.gif`);
    fs.writeFileSync(file, gif.bytes);
    const sec = timed.reduce((s, f) => s + f.ms, 0) / 1000;
    console.log(`${path.relative(process.cwd(), file)}：${w * 3}×${h * 3}，${sec.toFixed(1)} 秒，${Math.round(gif.bytes.length / 1024)} KB`);
  } finally {
    browser.close();
  }
}
