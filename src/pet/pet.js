// 形象窗口逻辑:订阅状态快照 → 切贴图 + 出气泡;点击穿透掩码;单击摸头/双击设置/拖动记忆。
// 渲染层不认识事件源,只认状态快照(架构硬规则)。

// 任何脚本级错误都要能看见,否则整页静默失效很难查
window.addEventListener('error', (e) => {
  try { window.__TAURI__.core.invoke('debug_log', { msg: `JS错误: ${e.message} @${e.filename}:${e.lineno}` }); } catch {}
});
window.addEventListener('unhandledrejection', (e) => {
  try { window.__TAURI__.core.invoke('debug_log', { msg: `未捕获拒绝: ${e.reason}` }); } catch {}
});

const { getCurrentWindow } = window.__TAURI__.window;
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const W = getCurrentWindow();
const stage = document.getElementById('stage');
const cat = document.getElementById('cat');
const bubble = document.getElementById('bubble');
const bubbleText = document.getElementById('bubble-text');
const bubbleRemark = document.getElementById('bubble-remark');

const MASK_N = 64;            // 与 Rust 侧 hit_through::N 一致
const ALPHA_MIN = 16;
const DOUBLE_CLICK_MS = 250;  // 双击判定窗
const DRAG_THRESHOLD = 4;     // 超过这个位移算拖动,不算点击
const TRANSIENT_MS = 5000;    // 短气泡停留
const BUSY_RELINE_MS = 60000; // busy 长跑换文案间隔

let lines = {};
let pack = null;
let cfg = null;
let snapshot = { state: 'idle', substate: 'none' };
let bubbleTimer = null;
let busyLineTimer = null;
let oneshotTimer = null;
let lastLine = {};
let visible = true;

// ---------- 形象包 ----------

async function loadPack(packId) {
  const res = await fetch(`../packs/${packId}/pack.json`);
  pack = await res.json();
  pack._dir = `../packs/${packId}`;
}

/** 状态 + 子状态 → 该播哪个文件 */
function assetFor(snap) {
  if (!pack) return null;
  const { state, substate } = snap;
  if (state === 'idle' && substate === 'sleep') return pack.extras?.sleep;
  if (state === 'greeting' && substate === 'wake') return pack.extras?.wake;
  return pack.states?.[state] ?? pack.states?.idle;
}

function setSprite(snap) {
  const asset = assetFor(snap);
  if (!asset) return;
  const src = `${pack._dir}/${asset.file}`;
  // 同一动图重设 src 会重播,一次性动画需要这个;循环动画避免无谓重播
  if (cat.getAttribute('src') === src && asset.loop !== 1) return;
  cat.style.opacity = '0';
  cat.setAttribute('src', src + (asset.loop === 1 ? `?t=${Date.now()}` : ''));
  requestAnimationFrame(() => { cat.style.opacity = '1'; });
}

// ---------- 点击穿透掩码 ----------

async function pushHitMask() {
  if (!cat.naturalWidth) return;
  // 掩码是"整个窗口"的 N×N 位图:窗口比形象大,余量部分一律透明(穿透)
  const winW = document.documentElement.clientWidth;
  const winH = document.documentElement.clientHeight;
  const r = cat.getBoundingClientRect();
  if (!winW || !winH || !r.width || !r.height) return;

  const off = document.createElement('canvas');
  off.width = off.height = MASK_N;
  const c = off.getContext('2d', { willReadFrequently: true });
  // 把形象按它在窗口中的实际位置画进掩码画布
  c.drawImage(
    cat,
    (r.left / winW) * MASK_N,
    (r.top / winH) * MASK_N,
    (r.width / winW) * MASK_N,
    (r.height / winH) * MASK_N,
  );
  const px = c.getImageData(0, 0, MASK_N, MASK_N).data;
  const bits = new Array(MASK_N * MASK_N);
  for (let i = 0; i < bits.length; i++) bits[i] = px[i * 4 + 3] > ALPHA_MIN;
  await invoke('set_hit_mask', { bits });
}

cat.addEventListener('load', pushHitMask);
window.addEventListener('resize', pushHitMask);

// ---------- 气泡 ----------

function pick(arr) {
  if (!arr?.length) return null;
  if (arr.length === 1) return arr[0];
  let v, guard = 0;
  do { v = arr[Math.floor(Math.random() * arr.length)]; } while (v === lastLine._v && ++guard < 8);
  lastLine._v = v;
  return v;
}

function lineFor(snap) {
  const { state, substate, detail } = snap;
  if (state === 'busy' && substate === 'company') return pick(lines.busy_company);
  if (state === 'busy') {
    const byTool = detail && lines.busy_tool?.[detail];
    return pick(byTool?.length ? byTool : lines.busy);
  }
  if (state === 'greeting' && substate === 'wake') return pick(lines.wake);
  if (state === 'idle' && substate === 'sleep') return null; // 打盹不说话
  // 等批权限 vs 等你回话,紧迫度不同,话也不同
  if (state === 'waiting') {
    return pick(snap.permission ? lines.waiting_permission : lines.waiting_input);
  }
  return pick(lines[state]);
}

/** remark 行:点名是哪个项目/什么工具在等你 —— waiting 最有用的信息 */
function remarkFor(snap) {
  // 常驻状态(等你决策 / 出错)永远要说清来源:
  // 这正是你需要知道"去哪个项目处理"的时刻,藏起来等于把功能做废了。
  // 「气泡备注」开关只管其余的日常状态。
  if (!snap.sticky && !cfg?.bubble_remark) return null;
  const parts = [];
  if (snap.session) parts.push(snap.session);
  if (snap.detail) parts.push(snap.detail);
  return parts.length ? parts.join(' · ') : null;
}

function showBubble(text, { sticky = false, remark = null } = {}) {
  if (!text) return hideBubble();
  bubbleText.textContent = text;
  if (remark) {
    bubbleRemark.textContent = remark;
    bubbleRemark.style.display = 'block';
  } else {
    bubbleRemark.style.display = 'none';
  }
  // 注意:不能写 display='' —— 那会回落到 CSS 里的 display:none
  bubble.style.display = 'block';
  requestAnimationFrame(() => bubble.classList.add('show'));
  clearTimeout(bubbleTimer);
  if (!sticky) bubbleTimer = setTimeout(hideBubble, TRANSIENT_MS);
}

function hideBubble() {
  clearTimeout(bubbleTimer);
  bubble.classList.remove('show');
  bubbleTimer = setTimeout(() => { bubble.style.display = 'none'; }, 180);
}

/** 气泡贴边翻转。
 *  方位判定与"翻转时补偿窗口位置"都在 Rust 侧一次做完(见 resolve_pet_layout):
 *  翻转会改变形象在窗口内的锚点,窗口不同步补偿的话形象会瞬间挪一大截 —— 就是"跳一下"。
 *  判定还必须相对形象所在那块屏,否则主屏上方的显示器(y 恒为负)会永远处于翻转态。 */
async function orientBubble() {
  try {
    const side = await invoke('resolve_pet_layout', {
      flipX: stage.classList.contains('flip-x'),
      flipY: stage.classList.contains('flip-y'),
    });
    if (!side) return;
    const changed =
      side.flip_x !== stage.classList.contains('flip-x') ||
      side.flip_y !== stage.classList.contains('flip-y');
    stage.classList.toggle('flip-x', side.flip_x);
    stage.classList.toggle('flip-y', side.flip_y);
    if (changed) pushHitMask(); // 形象在窗口内的位置变了,掩码要重算
  } catch { /* 取不到就维持当前方位 */ }
}

// ---------- 状态应用 ----------

function applySnapshot(snap) {
  snapshot = snap;
  setSprite(snap);
  clearInterval(busyLineTimer);

  const text = lineFor(snap);
  const remark = remarkFor(snap);
  if (snap.sticky) {
    const extra = snap.also_pending ? `（${lines.also_pending}）` : '';
    showBubble((text || '') + extra, { sticky: true, remark });
  } else {
    showBubble(text, { remark });
    if (snap.state === 'busy') {
      // 长跑时定期换一条,避免一句话挂到天荒地老
      busyLineTimer = setInterval(() => {
        if (snapshot.state === 'busy' && !snapshot.sticky) {
          showBubble(lineFor(snapshot), { remark: remarkFor(snapshot) });
        }
      }, BUSY_RELINE_MS);
    }
  }
  orientBubble();
  if (snap.state === 'waiting' && cfg?.chime) chime();
}

// ---------- 提示音 ----------

let audioCtx = null;
let chimedFor = null;
function chime() {
  // 同一次 waiting 只响一声
  const key = `${snapshot.session ?? ''}|${snapshot.detail ?? ''}`;
  if (chimedFor === key) return;
  chimedFor = key;
  try {
    audioCtx ??= new (window.AudioContext || window.webkitAudioContext)();
    const t = audioCtx.currentTime;
    const osc = audioCtx.createOscillator();
    const gain = audioCtx.createGain();
    osc.type = 'sine';
    osc.frequency.setValueAtTime(880, t);
    osc.frequency.exponentialRampToValueAtTime(1320, t + 0.08);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.12, t + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.45);
    osc.connect(gain).connect(audioCtx.destination);
    osc.start(t);
    osc.stop(t + 0.5);
  } catch { /* 音频不可用不影响主流程 */ }
}

// ---------- 交互:单击摸头 / 双击设置 / 拖动 ----------

let clickTimer = null;
let dragState = null;   // { winX, winY, startX, startY, moved }

// 拖动的坑与最终做法见 src-tauri/src/drag.rs 顶部注释。
cat.addEventListener('pointerdown', (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  cat.setPointerCapture(e.pointerId);
  // 同步建状态:快速点击可能早于 IPC 返回,异步建会把这一次点击整个吞掉
  const st = { startX: e.screenX, startY: e.screenY, moved: false, ready: false };
  dragState = st;
  invoke('start_pet_drag')
    .then(() => { st.ready = true; })
    .catch(() => { st.ready = false; });
});

// 窗口位置由原生事件监听驱动(见 src-tauri/src/drag.rs),前端只判断"动过没有",
// 用来区分点击与拖动。绝不能在这里每帧 invoke 去推位置:
// 松手时若还有一次调用在 IPC 路上,它会在松手之后才执行、读到已经移开的光标,
// 窗口就会"闪一下"跳过去追。
cat.addEventListener('pointermove', (e) => {
  const st = dragState;
  if (!st || st.moved) return;
  if (Math.hypot(e.screenX - st.startX, e.screenY - st.startY) > DRAG_THRESHOLD) {
    st.moved = true;
  }
});

async function endDrag(e) {
  if (!dragState) return;
  const moved = dragState.moved;
  dragState = null;
  try { cat.releasePointerCapture(e.pointerId); } catch { /* 已释放 */ }
  await invoke('end_pet_drag');

  if (moved) {          // 拖动过 → 不算点击
    orientBubble();
    pushHitMask();
    return;
  }
  if (clickTimer) {     // 第二击 → 双击开设置,取消摸头
    clearTimeout(clickTimer);
    clickTimer = null;
    invoke('open_settings_window', { page: null });
    return;
  }
  clickTimer = setTimeout(() => {
    clickTimer = null;
    petHead();
  }, DOUBLE_CLICK_MS);
}

cat.addEventListener('pointerup', endDrag);
cat.addEventListener('pointercancel', endDrag);

/** 摸头:播一遍亲昵动画后回到当前语义状态;不打断 waiting/alert 的常驻气泡 */
function petHead() {
  const asset = pack?.extras?.pet;
  if (!asset) return;
  clearTimeout(oneshotTimer);
  cat.setAttribute('src', `${pack._dir}/${asset.file}?t=${Date.now()}`);
  if (!snapshot.sticky) showBubble(pick(lines.pet));
  oneshotTimer = setTimeout(() => setSprite(snapshot), 1400);
}

// hover 气泡:随时能看当前状态
cat.addEventListener('mouseenter', () => {
  if (bubble.classList.contains('show')) return;
  const t = lineFor(snapshot);
  if (t) showBubble(t, { remark: remarkFor(snapshot) });
});

// ---------- 启动 ----------

async function refreshConfig() {
  const next = await invoke('get_config');
  const packChanged = cfg?.pack_id !== next.pack_id;
  const sizeChanged = cfg?.size !== next.size;
  cfg = next;
  if (sizeChanged) {
    stage.style.setProperty('--sprite', `${cfg.size}px`);
    requestAnimationFrame(pushHitMask);
  }
  if (packChanged) {
    await loadPack(cfg.pack_id);
    setSprite(snapshot);
    await pushHitMask();
  }
}

async function boot() {
  lines = await (await fetch('lines.json')).json();
  cfg = await invoke('get_config');
  stage.style.setProperty('--sprite', `${cfg.size}px`);
  await loadPack(cfg.pack_id);
  setSprite(snapshot);
  await pushHitMask();
  await orientBubble();

  await listen('state-changed', (e) => applySnapshot(e.payload));
  await listen('config-changed', refreshConfig);
  await listen('first-run-hint', () => {
    showBubble(lines.first_run, { sticky: false });
  });
  await listen('sprite-size', (e) => {
    stage.style.setProperty('--sprite', `${e.payload}px`);
    requestAnimationFrame(pushHitMask);
  });
  await listen('visibility', (e) => {
    visible = e.payload;
    // 隐藏时移除贴图停止解码(性能红线)
    if (!visible) cat.removeAttribute('src');
    else setSprite(snapshot);
  });
}

boot();
