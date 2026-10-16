// 簡單的 GIF89a 編碼器：全域調色盤、每格只編碼有變的範圍（沒變的像素用透明色），相同的格子合併，無限循環
// frames: [{ rgb: Uint8Array(w*h*3), ms }]，scale：每個像素放大幾倍（最近鄰）
export function encodeGif(frames, w, h, scale = 1) {
  // 調色盤：統計所有顏色，超過 255 色就把少見的顏色併到最接近的常見色（留一格當透明色）
  const count = new Map();
  for (const f of frames) for (let p = 0; p < f.rgb.length; p += 3) { const c = (f.rgb[p] << 16) | (f.rgb[p + 1] << 8) | f.rgb[p + 2]; count.set(c, (count.get(c) || 0) + 1); }
  const colors = [...count.keys()].sort((a, b) => count.get(b) - count.get(a));
  const keep = colors.slice(0, 255), index = new Map(keep.map((c, i) => [c, i]));
  const rgbOf = c => [c >> 16, (c >> 8) & 255, c & 255];
  for (const c of colors.slice(255)) {
    const [r, g, b] = rgbOf(c); let best = 0, bd = Infinity;
    keep.forEach((k, i) => { const [r2, g2, b2] = rgbOf(k), d = (r - r2) ** 2 + (g - g2) ** 2 + (b - b2) ** 2; if (d < bd) { bd = d; best = i; } });
    index.set(c, best);
  }
  const TRANS = keep.length;                           // 透明色的索引
  let bits = 1; while ((1 << bits) < TRANS + 1) bits++;
  const tableSize = 1 << bits;
  // 每格轉成索引（原解析度）
  const idx = frames.map(f => { const a = new Uint8Array(w * h); for (let i = 0; i < w * h; i++) a[i] = index.get((f.rgb[i * 3] << 16) | (f.rgb[i * 3 + 1] << 8) | f.rgb[i * 3 + 2]); return a; });
  // 相同的格子合併
  const merged = [];
  idx.forEach((a, i) => { const last = merged[merged.length - 1];
    if (last && a.every((v, j) => v === last.a[j])) last.ms += frames[i].ms; else merged.push({ a, ms: frames[i].ms }); });
  const out = [];
  const u16 = v => out.push(v & 255, (v >> 8) & 255);
  const W = w * scale, H = h * scale;
  out.push(...Buffer.from('GIF89a')); u16(W); u16(H);
  out.push(0x80 | ((bits - 1) << 4) | (bits - 1), 0, 0);   // 有全域調色盤
  for (let i = 0; i < tableSize; i++) { const c = i < keep.length ? rgbOf(keep[i]) : [0, 0, 0]; out.push(...c); }
  out.push(0x21, 0xff, 11, ...Buffer.from('NETSCAPE2.0'), 3, 1, 0, 0, 0);   // 無限循環
  let carry = 0;                                       // GIF 的延遲以 10ms 為單位：累積誤差，總長不跑掉
  merged.forEach((m, n) => {
    const prev = n ? merged[n - 1].a : null;
    let x0 = 0, y0 = 0, x1 = w - 1, y1 = h - 1;
    if (prev) {                                        // 只編碼有變的範圍
      x0 = w; y0 = h; x1 = -1; y1 = -1;
      for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (m.a[y * w + x] !== prev[y * w + x]) { if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y; }
    }
    const cs = Math.round((m.ms + carry) / 10); carry += m.ms - cs * 10;
    out.push(0x21, 0xf9, 4, (1 << 2) | (prev ? 1 : 0), cs & 255, cs >> 8, prev ? TRANS : 0, 0);   // 不清除、透明色
    const fw = (x1 - x0 + 1) * scale, fh = (y1 - y0 + 1) * scale;
    out.push(0x2c); u16(x0 * scale); u16(y0 * scale); u16(fw); u16(fh); out.push(0);
    const pix = new Uint8Array(fw * fh);
    for (let y = 0; y < fh; y++) for (let x = 0; x < fw; x++) {
      const sx = x0 + Math.floor(x / scale), sy = y0 + Math.floor(y / scale), v = m.a[sy * w + sx];
      pix[y * fw + x] = prev && v === prev[sy * w + sx] ? TRANS : v;
    }
    const min = Math.max(2, bits);
    out.push(min);
    const data = lzw(pix, min);
    for (let i = 0; i < data.length; i += 255) { const blk = data.slice(i, i + 255); out.push(blk.length, ...blk); }
    out.push(0);
  });
  out.push(0x3b);
  return { bytes: Uint8Array.from(out), frames: merged.length, colors: colors.length, palette: keep.length };
}

function lzw(pix, min) {                               // GIF 的 LZW：從低位元開始塞
  const clear = 1 << min, eoi = clear + 1, out = [];
  let size = min + 1, next = eoi + 1, dict = new Map(), cur = 0, nb = 0;
  const emit = c => { cur |= c << nb; nb += size; while (nb >= 8) { out.push(cur & 255); cur >>>= 8; nb -= 8; } };
  emit(clear);
  let prefix = pix[0];
  for (let i = 1; i < pix.length; i++) {
    const k = pix[i], key = prefix * 256 + k, c = dict.get(key);
    if (c !== undefined) { prefix = c; continue; }
    emit(prefix);
    if (next === 4096) { emit(clear); dict = new Map(); size = min + 1; next = eoi + 1; }
    else { if (next >= (1 << size)) size++; dict.set(key, next++); }
    prefix = k;
  }
  emit(prefix); emit(eoi);
  if (nb > 0) out.push(cur & 255);
  return out;
}
