#!/usr/bin/env node
/**
 * 抠透明:从画面边缘 flood fill 掉背景。
 *
 * 为什么不用 ffmpeg colorkey:colorkey 是**全局颜色匹配**,不管像素在不在角色身上。
 * 三花猫的白色身体 (255,255,255) 与奶油底 (246,238,221) 距离仅 ~0.15,
 * 会被连同背景一起抠掉 —— 表现为"猫身上破了个洞,能看到后面的桌面"。
 * flood fill 只抠与边界连通的区域,角色内部什么颜色都安全。
 *
 * 用法: node scripts/cutout.mjs <输入png> <输出png> [边长] [容差0-1]
 */
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export function cutout(src, dst, edge = 384, tol = 0.14) {
  const raw = join(tmpdir(), `dc-${process.pid}-${Math.random().toString(36).slice(2)}.raw`);
  // 缩放到目标边长并转成 rgba 原始数据
  execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', src,
    '-vf', `scale=${edge}:${edge}:flags=lanczos`, '-f', 'rawvideo', '-pix_fmt', 'rgba', raw]);
  const buf = readFileSync(raw);
  const W = edge, H = edge, N = W * H;

  // 背景参考色 = 四角平均(生成图的背景一律是纯色浅底)
  let br = 0, bg = 0, bb = 0;
  const corners = [0, W - 1, (H - 1) * W, N - 1];
  for (const i of corners) { br += buf[i * 4]; bg += buf[i * 4 + 1]; bb += buf[i * 4 + 2]; }
  br /= 4; bg /= 4; bb /= 4;

  const thr = tol * 255 * Math.sqrt(3);
  const near = (i) => {
    const dr = buf[i * 4] - br, dg = buf[i * 4 + 1] - bg, db = buf[i * 4 + 2] - bb;
    return Math.sqrt(dr * dr + dg * dg + db * db) <= thr;
  };

  // 从四边入队,BFS 只扩散到与背景色接近的连通像素
  const isBg = new Uint8Array(N);
  const queue = new Int32Array(N);
  let head = 0, tail = 0;
  const push = (i) => { if (!isBg[i] && near(i)) { isBg[i] = 1; queue[tail++] = i; } };
  for (let x = 0; x < W; x++) { push(x); push((H - 1) * W + x); }
  for (let y = 0; y < H; y++) { push(y * W); push(y * W + W - 1); }
  while (head < tail) {
    const i = queue[head++];
    const x = i % W, y = (i / W) | 0;
    if (x > 0) push(i - 1);
    if (x < W - 1) push(i + 1);
    if (y > 0) push(i - W);
    if (y < H - 1) push(i + W);
  }

  // 3×3 均值软化边界,避免硬锯齿
  const alpha = new Uint8Array(N);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      let sum = 0, cnt = 0;
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const nx = x + dx, ny = y + dy;
          if (nx < 0 || ny < 0 || nx >= W || ny >= H) { sum += 0; cnt++; continue; }
          sum += isBg[ny * W + nx] ? 0 : 255;
          cnt++;
        }
      }
      alpha[y * W + x] = Math.round(sum / cnt);
    }
  }
  for (let i = 0; i < N; i++) {
    buf[i * 4 + 3] = alpha[i];
    // 半透明边缘像素里残留的背景色会显出奶油色描边,压回中性
    if (alpha[i] === 0) { buf[i * 4] = 0; buf[i * 4 + 1] = 0; buf[i * 4 + 2] = 0; }
  }

  writeFileSync(raw, buf);
  execFileSync('ffmpeg', ['-y', '-loglevel', 'error',
    '-f', 'rawvideo', '-pix_fmt', 'rgba', '-s', `${W}x${H}`, '-i', raw, dst]);
  unlinkSync(raw);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [src, dst, edge, tol] = process.argv.slice(2);
  if (!src || !dst) { console.error('用法: cutout.mjs <in.png> <out.png> [边长] [容差]'); process.exit(1); }
  cutout(src, dst, Number(edge ?? 384), Number(tol ?? 0.14));
  console.log(`✓ ${dst}`);
}
