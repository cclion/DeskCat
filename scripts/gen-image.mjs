#!/usr/bin/env node
/**
 * 用 fal.ai 生成图片并存盘（设计 / UI 出图用）。从仓库根运行。
 *
 * 用法:
 *   node scripts/gen-image.mjs --prompt "..." --out apps/web/public/images/x.png [选项]
 *
 * 选项:
 *   --aspect   比例，默认 1:1。可选: 21:9 16:9 3:2 4:3 5:4 1:1 4:5 3:4 2:3 9:16
 *   --model    fal 模型 id，默认 fal-ai/nano-banana；高质量用 fal-ai/nano-banana-pro
 *   --n        张数，默认 1（>1 时文件名自动加 -1 -2 …）
 *   --format   png | jpeg | webp，默认 png
 *   --safety   1-6 内容安全档，1 最严（不传则用 fal 默认；被拒可调高）
 *
 * FAL_KEY 读取顺序: 环境变量 FAL_KEY > 仓库根 .env.local 里的 FAL_KEY=...
 *
 * 出图规则: 尽量不要在图里包含具体币种符号/金额（KSh、Rp 等）——产品要多国复用，
 *           烤进某国货币就不能跨国用。币种+金额用前端按 locale 叠加；必须时才烤进图。
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const argv = process.argv.slice(2);
const arg = (k, d) => {
  const i = argv.indexOf(`--${k}`);
  return i >= 0 ? argv[i + 1] : d;
};

const prompt = arg('prompt');
const out = arg('out');
if (!prompt || !out) {
  console.error('必填: --prompt "..." --out path/to/img.png');
  process.exit(1);
}
const model = arg('model', 'fal-ai/nano-banana');
const aspect = arg('aspect', '1:1');
const format = arg('format', 'png');
const n = parseInt(arg('n', '1'), 10);

function loadKey() {
  if (process.env.FAL_KEY) return process.env.FAL_KEY;
  const envFile = resolve(process.cwd(), '.env.local');
  if (existsSync(envFile)) {
    const m = readFileSync(envFile, 'utf8').match(/^\s*FAL_KEY\s*=\s*(.+)\s*$/m);
    if (m) return m[1].trim().replace(/^["']|["']$/g, '');
  }
  return null;
}
const FAL_KEY = loadKey();
if (!FAL_KEY) {
  console.error('缺少 FAL_KEY。在仓库根 .env.local 写一行: FAL_KEY=你的key');
  process.exit(1);
}

const body = { prompt, num_images: n, aspect_ratio: aspect, output_format: format };
if (argv.includes('--safety')) body.safety_tolerance = arg('safety', '4');

console.log(`→ ${model} | ${aspect} | ${n} 张`);
const res = await fetch(`https://fal.run/${model}`, {
  method: 'POST',
  headers: { Authorization: `Key ${FAL_KEY}`, 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
});
if (!res.ok) {
  console.error(`✗ fal ${res.status}: ${(await res.text()).slice(0, 600)}`);
  process.exit(1);
}
const data = await res.json();
const images = data.images ?? [];
if (!images.length) {
  console.error('✗ 无图返回:', JSON.stringify(data).slice(0, 400));
  process.exit(1);
}
for (let i = 0; i < images.length; i++) {
  const buf = Buffer.from(await (await fetch(images[i].url)).arrayBuffer());
  const path = resolve(images.length > 1 ? out.replace(/(\.\w+)$/, `-${i + 1}$1`) : out);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, buf);
  console.log(`✓ ${path}  (${(buf.length / 1024).toFixed(0)} KB)`);
}
