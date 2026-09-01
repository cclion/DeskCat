#!/usr/bin/env node
/**
 * 试验脚本:参考图 → 视频(Seedance image-to-video),用于评估"视频路线"是否可行。
 *
 * 用法:
 *   node scripts/gen-video.mjs --ref <图> --prompt "..." --out <mp4> \
 *        [--res 480p] [--dur 4] [--model bytedance/seedance-2.0/mini/image-to-video]
 *
 * 注意:视频按秒计价,跑之前先看清楚 --res 与 --dur —— 720p 是 480p 的两倍多。
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const argv = process.argv.slice(2);
const arg = (k, d) => { const i = argv.indexOf(`--${k}`); return i >= 0 ? argv[i + 1] : d; };
const ref = arg('ref'), prompt = arg('prompt'), out = arg('out');
if (!ref || !prompt || !out) {
  console.error('必填: --ref <图> --prompt "..." --out <输出.mp4>');
  process.exit(1);
}
const model = arg('model', 'bytedance/seedance-2.0/mini/image-to-video');
const resolution = arg('res', '480p');
const duration = arg('dur', '4');

function loadKey() {
  if (process.env.FAL_KEY) return process.env.FAL_KEY;
  const f = resolve(process.cwd(), '.env.local');
  if (existsSync(f)) {
    const m = readFileSync(f, 'utf8').match(/^\s*FAL_KEY\s*=\s*(.+)\s*$/m);
    if (m) return m[1].trim().replace(/^["']|["']$/g, '');
  }
  return null;
}
const FAL_KEY = loadKey();
if (!FAL_KEY) { console.error('缺少 FAL_KEY'); process.exit(1); }

const buf = readFileSync(resolve(ref));
const ext = ref.toLowerCase().endsWith('.webp') ? 'webp' : ref.toLowerCase().endsWith('.jpg') ? 'jpeg' : 'png';
const body = {
  prompt,
  image_url: `data:image/${ext};base64,${buf.toString('base64')}`,
  resolution,
  duration,
  aspect_ratio: '1:1',
  generate_audio: false,   // 桌宠不要声音,也省钱
};

const est = { '480p': 0.0721, '720p': 0.1547 }[resolution] * Number(duration);
console.log(`→ ${model} | ${resolution} ${duration}s | 预计 $${est.toFixed(3)}`);

const t0 = Date.now();
const res = await fetch(`https://fal.run/${model}`, {
  method: 'POST',
  headers: { Authorization: `Key ${FAL_KEY}`, 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
  signal: AbortSignal.timeout(600_000),
});
if (!res.ok) {
  console.error(`✗ fal ${res.status}: ${(await res.text()).slice(0, 600)}`);
  process.exit(1);
}
const data = await res.json();
const url = data?.video?.url;
if (!url) { console.error('✗ 无视频返回:', JSON.stringify(data).slice(0, 400)); process.exit(1); }

const vb = Buffer.from(await (await fetch(url, { signal: AbortSignal.timeout(300_000) })).arrayBuffer());
const path = resolve(out);
mkdirSync(dirname(path), { recursive: true });
writeFileSync(path, vb);
console.log(`✓ ${path} (${(vb.length / 1024).toFixed(0)} KB, 耗时 ${((Date.now()-t0)/1000).toFixed(0)}s, seed=${data.seed})`);
