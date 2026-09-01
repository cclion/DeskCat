#!/usr/bin/env node
/**
 * 参考图编辑式生成：一张参考图 + 指令 → 保持角色一致的新图（fal.ai nano-banana/edit）。
 *
 * 用法:
 *   node scripts/gen-edit.mjs --ref assets/xxx.png --prompt "..." --out assets/yyy.png [--model fal-ai/nano-banana/edit]
 *
 * FAL_KEY 读取顺序: 环境变量 FAL_KEY > 仓库根 .env.local 里的 FAL_KEY=...
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const argv = process.argv.slice(2);
const arg = (k, d) => {
  const i = argv.indexOf(`--${k}`);
  return i >= 0 ? argv[i + 1] : d;
};

const ref = arg('ref');
const prompt = arg('prompt');
const out = arg('out');
if (!ref || !prompt || !out) {
  console.error('必填: --ref 参考图 --prompt "..." --out 输出路径');
  process.exit(1);
}
const model = arg('model', 'fal-ai/nano-banana/edit');

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

const refBuf = readFileSync(resolve(ref));
const ext = ref.toLowerCase().endsWith('.webp') ? 'webp' : ref.toLowerCase().endsWith('.jpg') || ref.toLowerCase().endsWith('.jpeg') ? 'jpeg' : 'png';
const dataUri = `data:image/${ext};base64,${refBuf.toString('base64')}`;

const body = { prompt, image_urls: [dataUri], num_images: 1, output_format: 'png' };

console.log(`→ ${model} | ref: ${ref}`);
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
const buf = Buffer.from(await (await fetch(images[0].url)).arrayBuffer());
const path = resolve(out);
mkdirSync(dirname(path), { recursive: true });
writeFileSync(path, buf);
console.log(`✓ ${path}  (${(buf.length / 1024).toFixed(0)} KB)`);
