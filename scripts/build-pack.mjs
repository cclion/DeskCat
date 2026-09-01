#!/usr/bin/env node
/**
 * 一键出形象包:参考图 → 六态+extras 立绘 → 每态微变帧 → 抠透明 → 动态 WebP → pack.json
 *
 * 用法:
 *   node scripts/build-pack.mjs --ref assets/形象候选/cat-b.png --id calico --name 三花猫
 *   node scripts/build-pack.mjs --id calico --redo waiting        # 单态重 roll(需 work 目录里有该包)
 *
 * 断点续跑:中间产物在 .packwork/<id>/,已存在的文件自动跳过;失败后重跑即续。
 * 出包目录 = 前端资源根(src/packs/),内置形象包随前端一起打包,无需 asset 协议。
 * FAL_KEY 读取: 环境变量 > .env.local
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync, rmSync, readdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { cutout } from './cutout.mjs';

// ---------- 参数 ----------
const argv = process.argv.slice(2);
const arg = (k, d) => { const i = argv.indexOf(`--${k}`); return i >= 0 ? argv[i + 1] : d; };
const packId = arg('id');
const refImg = arg('ref');
const packName = arg('name', packId);
const outRoot = arg('out', 'src/packs');
const redo = arg('redo');
const tolerance = Number(arg('tolerance', 0.14)); // flood fill 判定背景的颜色容差
const species = arg('species', 'cat');       // 一致性提示词按物种切换
const quality = Number(arg('quality', 72));  // WebP 质量
// 输出边长:形象最大显示 220pt(Retina 440px),384 已有余量;512 是过采样,白吃体积
const edge = Number(arg('edge', 384));
if (!packId) { console.error('必填: --id <pack-id>;首次出包还需 --ref <参考图>'); process.exit(1); }

// ---------- 状态表(提示词 = 管线验证期原文 + 防镜像追加) ----------
// 物种相关的"保持一致"描述:换形象包只需在这里加一条,状态表本身与物种无关
const SPECIES = {
  cat: {
    noun: 'cat',
    marks: 'same calico color patches in the same places',
    laptop: 'front paws', hands: 'front paws', ears: 'ears', tail: 'tail',
  },
  capybara: {
    noun: 'capybara',
    marks: 'same warm brown fur tone and the same muzzle and eye placement',
    laptop: 'front paws', hands: 'front paws', ears: 'small round ears', tail: 'rump',
  },
};
const SP = SPECIES[species] ?? SPECIES.cat;
const KEEP_BASE = `Keep the exact same ${SP.noun} character as the reference image: ${SP.marks}, same face, same thick outlines, same flat sticker style, same light cream background, single character, no text.`;
const KEEP_FRAME = `Keep the exact same ${SP.noun} character and the exact same pose and camera angle as the reference image: ${SP.marks}, same thick outlines, same flat sticker style, same light cream background, single character, no text. Change ONLY:`;
const NO_MIRROR = ' Do not mirror or flip the character: the markings must stay on the same sides as the reference image.';

const STATES = {
  idle: {
    base: `The ${SP.noun} is now sitting upright, relaxed, eyes half open, idle daydreaming.`,
    frames: [
      'the eyes are now fully closed in a content blink, everything else identical.',
      'the tail is now curled slightly upward and one ear tilted a bit, eyes half open, everything else identical.',
    ],
    sequence: [1, 2, 1, 3], duration: 500, loop: 0,
  },
  busy: {
    base: `The ${SP.noun} is now lying down typing busily on a tiny laptop keyboard with front paws, focused expression.`,
    frames: [
      'the front paws are now in a different typing position on the keyboard, everything else identical.',
      'the head is now tilted slightly down looking at the keyboard, everything else identical.',
    ],
    sequence: [1, 2, 3, 2], duration: 400, loop: 0,
  },
  waiting: {
    base: `The ${SP.noun} is now leaning forward eagerly and waving one front paw at the viewer, wide expectant eyes, trying to get attention.`,
    frames: [
      'the waving paw is now at a lower position mid-wave, everything else identical.',
      'the ears are now perked up higher and the eyes slightly wider, everything else identical.',
    ],
    sequence: [1, 2, 1, 3], duration: 450, loop: 0,
  },
  celebrating: {
    base: `The ${SP.noun} is now standing on hind legs cheering joyfully with both front paws raised up, happy open-mouth smile, small sparkles around.`,
    frames: [
      'both raised paws are now slightly lower mid-bounce and the sparkles are in different positions, everything else identical.',
      'the mouth is now closed in a big satisfied grin, paws fully raised, everything else identical.',
    ],
    sequence: [1, 2, 1, 3], duration: 400, loop: 1,
  },
  alert: {
    base: `The ${SP.noun} is now startled with fur puffed up, arched back, wide shocked eyes, alarmed.`,
    frames: [
      'the fur is now puffed even more and the pupils are tiny dots, everything else identical.',
    ],
    sequence: [1, 2], duration: 250, loop: 0,
  },
  greeting: {
    base: `The ${SP.noun} is now sitting and waving one front paw in a friendly hello, warm smile, other paw on the ground.`,
    frames: [
      'the waving paw is now lower, mid-wave, everything else identical.',
      'the eyes are now happily closed in a smile, waving paw raised high, everything else identical.',
    ],
    sequence: [1, 2, 3, 2], duration: 450, loop: 1,
  },
};

const EXTRAS = {
  sleep: {
    base: `The ${SP.noun} is now curled up lying down fast asleep, eyes fully closed, peaceful content expression.`,
    frames: [
      'the curled body is now slightly flatter as if breathing out, everything else identical.',
    ],
    sequence: [1, 2], duration: 900, loop: 0,
  },
  pet: {
    base: `The ${SP.noun} is now squinting happily with blushing cheeks, head tilted slightly up as if enjoying being petted, content smile.`,
    frames: [
      'the eyes are now fully squeezed shut with an even bigger happy smile, everything else identical.',
    ],
    sequence: [1, 2, 1], duration: 400, loop: 1,
  },
  wake: {
    base: `The ${SP.noun} is now stretching with front paws extended forward and rear up, mouth open in a big yawn, eyes squeezed shut.`,
    frames: [
      'the stretch is now released, sitting up with eyes half open and a sleepy expression, everything else identical.',
    ],
    sequence: [1, 2], duration: 600, loop: 1,
  },
};

// ---------- fal 调用(重试 ≤2) ----------
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
if (!FAL_KEY) { console.error('缺少 FAL_KEY(.env.local)'); process.exit(1); }

async function genEdit(refPath, prompt, outPath, label) {
  const dataUri = `data:image/png;base64,${readFileSync(refPath).toString('base64')}`;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      const res = await fetch('https://fal.run/fal-ai/nano-banana/edit', {
        method: 'POST',
        headers: { Authorization: `Key ${FAL_KEY}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ prompt, image_urls: [dataUri], num_images: 1, output_format: 'png' }),
        signal: AbortSignal.timeout(120_000),
      });
      if (!res.ok) throw new Error(`fal ${res.status}: ${(await res.text()).slice(0, 300)}`);
      const data = await res.json();
      if (!data.images?.length) throw new Error(`无图返回: ${JSON.stringify(data).slice(0, 200)}`);
      const buf = Buffer.from(await (await fetch(data.images[0].url, { signal: AbortSignal.timeout(60_000) })).arrayBuffer());
      writeFileSync(outPath, buf);
      console.log(`  ✓ ${label} (${(buf.length / 1024).toFixed(0)} KB)`);
      return;
    } catch (e) {
      console.warn(`  ⚠ ${label} 第${attempt}次失败: ${e.message}`);
      if (attempt === 3) throw new Error(`${label} 重试耗尽,已完成部分保留在 work/,修复后重跑续做`);
      await new Promise(r => setTimeout(r, 3000 * attempt));
    }
  }
}

// ---------- 本地处理 ----------
// 抠透明走 flood fill,不用 colorkey:
// colorkey 是全局颜色匹配,会把角色身上与背景接近的浅色(三花猫的白肚子)一起抠掉。
function transparent(src, dst) {
  cutout(src, dst, edge, tolerance);
}
function toWebp(frames, duration, loop, dst) {
  const args = ['-loop', String(loop), '-q', String(quality)];
  for (const f of frames) args.push('-d', String(duration), f);
  args.push('-o', dst);
  execFileSync('img2webp', args, { stdio: ['ignore', 'ignore', 'inherit'] });
}

// ---------- 主流程 ----------
const packDir = resolve(outRoot, packId);
// 中间产物放仓库外层的 .packwork/,不能放在前端资源根里
// —— frontendDist 会把整个 src/ 嵌进二进制,work/ 会让包体积膨胀几十 MB
const workDir = resolve(arg('work', '.packwork'), packId);
const extrasDir = join(packDir, 'extras');
mkdirSync(workDir, { recursive: true });
mkdirSync(extrasDir, { recursive: true });

const ALL = { ...STATES, ...EXTRAS };
if (redo && !ALL[redo]) { console.error(`未知状态: ${redo}(可选: ${Object.keys(ALL).join('/')})`); process.exit(1); }
if (redo) {
  for (const f of readdirSync(workDir)) if (f.startsWith(`${redo}-`)) rmSync(join(workDir, f));
  const webp = STATES[redo] ? join(packDir, `${redo}.webp`) : join(extrasDir, `${redo}.webp`);
  if (existsSync(webp)) rmSync(webp);
  console.log(`♻ 重做 ${redo}`);
}

const refResolved = refImg ? resolve(refImg) : join(workDir, '_ref.png');
if (refImg) {
  if (!existsSync(refResolved)) { console.error(`参考图不存在: ${refImg}`); process.exit(1); }
  writeFileSync(join(workDir, '_ref.png'), readFileSync(refResolved));
} else if (!existsSync(refResolved)) {
  console.error('work 目录无参考图,首次出包请带 --ref'); process.exit(1);
}

for (const [state, def] of Object.entries(ALL)) {
  const isExtra = !!EXTRAS[state];
  const webpPath = isExtra ? join(extrasDir, `${state}.webp`) : join(packDir, `${state}.webp`);
  if (existsSync(webpPath)) { console.log(`⏭ ${state} 已存在,跳过`); continue; }
  console.log(`▶ ${state}`);

  // 1) 基帧:参考图 → 该状态立绘
  const f1 = join(workDir, `${state}-f1.png`);
  if (!existsSync(f1)) await genEdit(join(workDir, '_ref.png'), `${KEEP_BASE} ${def.base}${NO_MIRROR}`, f1, `${state} 基帧`);

  // 2) 微变帧:基帧 → 姿态微变
  const framePngs = [f1];
  for (let i = 0; i < def.frames.length; i++) {
    const fn = join(workDir, `${state}-f${i + 2}.png`);
    if (!existsSync(fn)) await genEdit(f1, `${KEEP_FRAME} ${def.frames[i]}${NO_MIRROR}`, fn, `${state} 帧${i + 2}`);
    framePngs.push(fn);
  }

  // 3) 抠透明 + 缩放
  const tPngs = framePngs.map((p, i) => {
    const t = join(workDir, `${state}-t${i + 1}.png`);
    if (!existsSync(t)) transparent(p, t);
    return t;
  });

  // 4) 按 sequence 拼动态 WebP
  toWebp(def.sequence.map(n => tPngs[n - 1]), def.duration, def.loop, webpPath);
  console.log(`  ✓ ${state}.webp(${def.sequence.length} 帧)`);
}

// 5) pack.json + 包清单(Rust 侧靠它枚举,不能依赖运行时扫目录)
const packJson = {
  id: packId,
  name: packName,
  version: '1.0.0',
  states: Object.fromEntries(Object.keys(STATES).map(s => [s, { file: `${s}.webp`, loop: STATES[s].loop }])),
  extras: Object.fromEntries(Object.keys(EXTRAS).map(s => [s, { file: `extras/${s}.webp`, loop: EXTRAS[s].loop }])),
};
writeFileSync(join(packDir, 'pack.json'), JSON.stringify(packJson, null, 2) + '\n');

// 更新清单:列出 outRoot 下所有含 pack.json 的目录
const indexPath = resolve(outRoot, 'index.json');
const ids = readdirSync(resolve(outRoot), { withFileTypes: true })
  .filter(d => d.isDirectory() && existsSync(join(resolve(outRoot), d.name, 'pack.json')))
  .map(d => d.name)
  .sort();
writeFileSync(indexPath, JSON.stringify(ids, null, 2) + '\n');

// 6) 自检:文件齐全且非空
let ok = true;
for (const [s, v] of [...Object.entries(packJson.states), ...Object.entries(packJson.extras)]) {
  const p = join(packDir, v.file);
  if (!existsSync(p) || readFileSync(p).length === 0) { console.error(`✗ 缺失/空文件: ${v.file}`); ok = false; }
}
// 体积红线:单文件 ≤250KB、整包 ≤2MB(桌宠常驻,资源不能膨胀)
const MAX_FILE = 250 * 1024, MAX_PACK = 2 * 1024 * 1024;
let total = 0;
for (const [, v] of [...Object.entries(packJson.states), ...Object.entries(packJson.extras)]) {
  const n = readFileSync(join(packDir, v.file)).length;
  total += n;
  if (n > MAX_FILE) {
    console.error(`✗ ${v.file} 体积 ${(n / 1024).toFixed(0)}KB 超过 250KB;用更低的 --quality 重跑`);
    ok = false;
  }
}
if (total > MAX_PACK) {
  console.error(`✗ 整包 ${(total / 1024).toFixed(0)}KB 超过 2MB;用更低的 --quality 重跑`);
  ok = false;
}
if (!ok) process.exit(1);
console.log(`\n✅ ${packId} 出包完成 → ${packDir}(整包 ${(total / 1024).toFixed(0)}KB;中间产物在 ${workDir})`);
