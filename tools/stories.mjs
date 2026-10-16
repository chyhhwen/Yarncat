/* README 的 GIF 劇本：在頁面裡執行，可以用 index.html 自己的函式、變數（setState、cat、box……）和 recorder.mjs 的 R。
   width 是視窗寬（CSS 像素，100% 縮放時每 3 個 = 1 格）；seed 是亂數種子：貓的動作有亂數，換種子錄出來就不一樣。
   box、tree：世界寬 640 格，只裁 117 格寬（放大 3 倍是 351×150，跟展示表格裡其他 GIF 一樣大）；
   hero：整條 252 格都錄（756×150）；visit：兩個 160 格寬的視窗並排 */

export const hero = {                                  // 游標把毛球往上一甩，貓去追、壓低、飛撲，一路追到紙箱、再追回貓跳台
  width: 756, seed: 12,
  setup: `R.calm(); toy = 'ball';
    cat.x = W * 0.35; cat.y = FLOOR; cat.dir = 1; cat.energy = 85; setState('sit', 1e9);
    ball.x = W * 0.6; ball.y = FLOOR - ball.r; ball.vx = ball.vy = 0; initRope();
    tree.x = clamp(W * 0.07, HALF.tree, W - HALF.tree); litter.x = clamp(W * 0.2, HALF.litter, W - HALF.litter);
    box.x = clamp(W * 0.88, HALF.box, W - HALF.box); tree.y = litter.y = box.y = FLOOR; initPom();
    R.crop = { x0: 0, w: W }; R.tick(30);`,
  script: `
    R.move(212, 14);
    R.step(12);
    R.glide(ball.x, ball.y - 1, 16);                   // 游標移到毛球上
    R.step(4);
    R.down();                                          // 抓住毛球往右上輕輕一甩
    R.glide(ball.x + 4, ball.y - 16, 7, false);
    R.up();
    R.glide(242, 12, 24);                              // 游標移到右上角待著，剩下的交給貓
    R.step(500 - R.ticks);
  `,
};

export const box = {                                   // 貓溜進紙箱，游標靠近時探頭偷看；游標在紙箱上來回摸，貓碰一聲蹦出來，炸毛跳開
  width: 1920, seed: 3,
  setup: `R.calm(); toy = 'ball'; ball.x = 600; ball.vx = ball.vy = 0; initRope();
    tree.x = 60; initPom(); litter.x = 130; box.x = 347; box.y = FLOOR;
    cat.x = 397; cat.y = FLOOR; cat.dir = -1; cat.energy = 80; setState('sit', 1e9);
    mouse.x = 430; mouse.y = 20; R.crop = { x0: 300, w: 117 }; R.tick(30);`,
  script: `
    R.step(30);                                        // 坐著
    boxCD = 0; nextIdle();                             // 想躲紙箱：壓低身體溜過去、跳進去
    R.until(() => cat.state === 'hide', 400);
    cat.t = 160;                                       // 躲好不久就探頭
    R.step(8);
    R.move(416, 22);                                   // 游標從右邊進來，貓探頭偷看（眼睛跟著游標）
    R.glide(box.x + 16, 30, 50);
    R.step(18);
    R.glide(box.x + 11, 36, 8);                        // 移到紙箱上，來回摸
    R.glide(box.x + 1, 36, 8, false);
    R.glide(box.x + 11, 36, 8, false);
    R.glide(box.x + 1, 36, 8, false);                  // 第二次換方向：碰！
    R.step(8);
    R.glide(box.x - 30, 16, 30);                       // 嚇到，游標往左上縮
    R.until(() => cat.state === 'spook', 200);
    R.until(() => cat.t >= cat.dur - 3, 200);          // 炸毛跳完就收尾（不錄後面的暴衝）
  `,
};

export const tree = {                                  // 貓走去磨爪；游標點一下平台，小毛球晃起來，貓跑過去撥；撥完跳上平台趴著
  width: 1920, seed: 5,
  setup: `R.calm(); toy = 'ball'; ball.x = 600; ball.vx = ball.vy = 0; initRope();
    litter.x = 130; box.x = 520; tree.x = 337; tree.y = FLOOR; initPom();
    cat.x = 385; cat.y = FLOOR; cat.dir = -1; cat.energy = 80; setState('sit', 1e9);
    R.crop = { x0: 300, w: 117 }; R.tick(30);`,
  script: `
    R.step(10);
    goTree('scratch');                                 // 走去貓跳台磨爪
    R.until(() => cat.state === 'scratch', 300);
    cat.dur = 1e9;                                     // 磨到被小毛球吸引為止
    R.step(40);
    R.move(416, 6);                                    // 游標從右上角進來，點平台左半邊
    R.glide(tree.x - 8, FLOOR - 17, 36);
    R.step(6);
    R.down(); R.up();                                  // 點一下：小毛球晃起來，貓跑過去撥
    R.glide(tree.x - 22, 12, 30);                      // 游標移開
    R.until(() => cat.state === 'pom', 120);
    cat.dur = 180;                                     // 撥三秒
    R.until(() => cat.state !== 'pom', 400);
    cat.energy = 35; climb();                          // 玩完跳上平台趴著
    R.until(() => cat.state === 'rest', 60);
    R.step(70);
  `,
};

/* 串門子：兩個並排的視窗互當隔壁。左邊的橘貓走出右邊 → 從右邊視窗的左邊走進來 → 跟灰貓碰鼻子 →
   右邊的人摸摸客人 → 客人走回去 → 橘貓回家坐下。毛球藏在畫面外（拿在手上就不會被夾回畫面裡） */
const hideBall = "toy = 'ball'; ball.held = true; ball.x = W + 60; ball.y = FLOOR - ball.r; ball.vx = ball.vy = 0; initRope();";
export const visit = {
  pair: true, width: 480, seed: 5,                     // 左邊視窗用 seed，右邊用 seed + 1
  left: `R.calm(); myCoat = 'orange'; coatPicked = true; ${hideBall}
    setFurniture('litter', false); setFurniture('box', false); tree.x = 22; initPom(); for (let i = 0; i < 60; i++) updateFurniture();
    cat.x = 112; cat.dir = 1; cat.energy = 80; setState('sit', 1e9); R.crop = { x0: 0, w: W };`,
  right: `R.calm(); myCoat = 'gray'; coatPicked = true; ${hideBall}
    setFurniture('litter', false); setFurniture('tree', false); box.x = 138;
    cat.x = 92; cat.dir = -1; cat.energy = 80; setState('sit', 1e9); R.crop = { x0: 0, w: W };
    // 每格：灰貓碰完鼻子就坐在客人旁邊；客人坐下來後，游標從上面伸過來摸摸牠，摸完一會兒客人就回家
    window.__pet = { t0: -1 };
    window.__each = () => {
      const P = __pet;
      if (P.boop && cat.state !== 'boop' && cat.state !== 'sit') setState('sit', 1e9);
      if (cat.state === 'boop') P.boop = true;
      if (P.t0 < 0 && guest && ['sit', 'lick', 'walk', 'sleep'].includes(guest.state) && cat.state !== 'boop') { P.t0 = R.ticks; guestSet('sit', 1e9); P.gx = guest.x; }
      if (P.t0 < 0) return;
      const k = R.ticks - P.t0, gx = P.gx;
      if (k === 0) R.move(gx + 30, 4);
      else if (k < 30) R.move(gx + 30 - (30 - 4) * k / 30, 4 + (FLOOR - 14 - 4) * k / 30);
      else if (k < 120) R.move(gx + 4 + Math.round(5 * Math.sin((k - 30) / 5)), FLOOR - 14);   // 在背上來回摸
      else if (k < 150) R.move(gx + 4 + (k - 120), FLOOR - 14 - (k - 120) * 0.8);
      else if (k === 210) { guest.stay = 0; guestSet('sit', 1); }
    };`,
  start: { left: 'cat.t = 0; R.step(30); visitCD = 0; nextIdle();', right: 'R.step(30, __each)' },   // 先坐一下，然後想去隔壁
  step: { left: 'R.step(3)', right: 'R.step(3, __each)' },
  done: "['sit', 'lick'].includes(cat.state) && R.log.some(l => l.s === 'arrive')",   // 在左邊視窗判斷：貓回家坐下了
  tail: 20,                                            // 回家後再坐一下
};
