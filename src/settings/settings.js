// 设置窗口:唯一配置中枢。所有改动即时生效 + 落盘 + 与菜单栏同步。
// 只用组件库的类,不新增一次性样式。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const STATE_WORDS = {
  idle: '待机中', busy: '陪伴中', waiting: '等你中',
  celebrating: '庆祝中', alert: '出状况了', greeting: '打招呼',
};

let cfg = null;
let packs = [];
let previewId = null;   // 正在预览(未必已应用)的形象

const PREVIEW_STATES = [
  ['idle', '待机'], ['busy', '干活'], ['waiting', '等你'],
  ['celebrating', '庆祝'], ['alert', '出错'], ['greeting', '打招呼'],
];
const PREVIEW_EXTRAS = [['sleep', '打盹'], ['pet', '摸头'], ['wake', '睡醒']];

// ---------- 配置写入 ----------

async function put(key, value, errBox) {
  try {
    cfg = await invoke('update_config', { key, value });
    if (errBox) errBox.style.display = 'none';
    return true;
  } catch (e) {
    if (errBox) { errBox.textContent = String(e); errBox.style.display = 'block'; }
    return false;
  }
}

// ---------- 导航 ----------

function gotoPage(page) {
  document.querySelectorAll('.nav .ni').forEach((n) =>
    n.classList.toggle('sel', n.dataset.page === page));
  document.querySelectorAll('.page').forEach((p) =>
    p.classList.toggle('active', p.id === `page-${page}`));
}

$('nav').addEventListener('click', (e) => {
  const ni = e.target.closest('.ni');
  if (ni) gotoPage(ni.dataset.page);
});

// ---------- 形象卡片墙 ----------

function renderPacks() {
  const wall = $('pcards');
  wall.innerHTML = '';
  if (!packs.length) {
    wall.innerHTML = '<div class="empty">没有找到可用的形象包</div>';
  }
  previewId ??= cfg.pack_id;
  for (const p of packs) {
    const active = p.id === cfg.pack_id;
    const card = document.createElement('div');
    card.className = 'pcard' + (p.id === previewId ? ' sel' : '');
    card.innerHTML = `
      <img src="../packs/${p.id}/${p.states.idle.file}" alt="" />
      <b></b>
      <span class="${active ? 'use' : 'apply'}">${active ? '当前使用 ✓' : '应用'}</span>`;
    card.querySelector('b').textContent = p.name;
    // 点卡片 = 先预览各状态,不改桌面上的猫
    card.addEventListener('click', () => {
      previewId = p.id;
      renderPacks();
    });
    if (!active) {
      // 点「应用」才真正换形象
      card.querySelector('.apply').addEventListener('click', async (e) => {
        e.stopPropagation();
        if (await put('pack_id', p.id)) { previewId = p.id; renderPacks(); }
      });
    }
    wall.appendChild(card);
  }
  // v1.5 自定义生成入口的占位
  const ghost = document.createElement('div');
  ghost.className = 'pcard ghost';
  ghost.innerHTML = '<div class="plus">＋</div><b>自定义形象</b><span class="soon">敬请期待</span>';
  ghost.addEventListener('click', () => { /* v1.5 才开放 */ });
  wall.appendChild(ghost);
  renderPreview();
  syncCurrentPet();
}

/** 选中的形象有哪几种状态,长什么样 —— 看清楚再决定应用 */
function renderPreview() {
  const box = $('preview');
  const p = packs.find((x) => x.id === previewId);
  if (!p) { box.hidden = true; return; }
  box.hidden = false;
  $('pv-name').textContent = p.name;
  $('pv-hint').textContent =
    p.id === cfg.pack_id ? '正在使用 · 以下是它的全部状态' : '预览 · 觉得合适再点「应用」';

  const strip = $('pv-strip');
  strip.innerHTML = '';
  const items = [
    ...PREVIEW_STATES.map(([k, label]) => [p.states?.[k], label]),
    ...PREVIEW_EXTRAS.map(([k, label]) => [p.extras?.[k], label]),
  ];
  for (const [asset, label] of items) {
    if (!asset) continue;
    const cell = document.createElement('div');
    cell.className = 'pv';
    // 加时间戳让一次性动画(庆祝/打招呼)每次渲染都重播一遍
    cell.innerHTML = `<img src="../packs/${p.id}/${asset.file}?t=${Date.now()}" alt="" /><i></i>`;
    cell.querySelector('i').textContent = label;
    strip.appendChild(cell);
  }
}

function syncCurrentPet() {
  const p = packs.find((x) => x.id === cfg.pack_id);
  if (!p) return;
  $('cur-avatar').src = `../packs/${p.id}/${p.states.idle.file}`;
  $('cur-name').textContent = p.name;
}

// ---------- 尺寸滑杆 ----------

function bindSlider() {
  const el = $('size-slider');
  const min = Number(el.dataset.min), max = Number(el.dataset.max);
  const paint = (v) => el.style.setProperty('--fill', `${((v - min) / (max - min)) * 100}%`);
  const apply = (e) => {
    const r = el.getBoundingClientRect();
    const pct = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const v = Math.round(min + pct * (max - min));
    paint(v);
    put('size', v); // 拖动中即时预览:桌面形象同步缩放
  };
  // 轨道只有 4px 高,监听挂 document 才拖得住
  el.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    apply(e);
    const move = (ev) => apply(ev);
    const up = () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
    };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
  });
  el._paint = paint;
}

// ---------- 开关与步进器 ----------

function bindToggles() {
  document.querySelectorAll('.tg[data-cfg]').forEach((el) => {
    el.addEventListener('click', async () => {
      const key = el.dataset.cfg;
      const next = !el.classList.contains('on');
      el.classList.toggle('on', next);
      const errBox = el.closest('.page')?.querySelector('.err');
      if (!(await put(key, next, errBox))) {
        el.classList.toggle('on', !next); // 写入失败回落,不假装成功
      }
    });
  });
}

function bindSteppers() {
  document.querySelectorAll('.step[data-cfg]').forEach((el) => {
    const key = el.dataset.cfg;
    const min = Number(el.dataset.min), max = Number(el.dataset.max);
    el.querySelectorAll('.pm').forEach((btn) => {
      btn.addEventListener('click', async () => {
        const cur = Number(cfg?.[key] ?? min);
        const next = Math.min(max, Math.max(min, cur + (btn.dataset.dir === 'up' ? 1 : -1)));
        if (next === cur) return;
        if (await put(key, next)) el.querySelector('.val').textContent = `${next} 分钟`;
      });
    });
  });
}

// ---------- Claude Code 连接 ----------

async function renderConnection() {
  let conn;
  try {
    conn = await invoke('get_connection');
  } catch { return; }
  const connected = conn.installed && conn.listening;
  $('conn-dot').className = 'dot ' + (connected ? 'on' : 'off');
  $('conn-label').textContent = connected ? '已连接' : '未连接';
  $('conn-hint').textContent = connected
    ? `hooks 已写入 ${conn.settings_path}`
    : (conn.listening ? '写入 hooks 后才能感知' : `本地端口被占用,感知不可用`);

  const action = $('conn-action');
  action.innerHTML = '';
  if (connected) {
    const n = document.createElement('span');
    n.className = 'mono';
    n.textContent = `${conn.sessions} 个会话活跃`;
    action.appendChild(n);
    const off = document.createElement('span');
    off.className = 'btn dark';
    off.style.marginLeft = '10px';
    off.textContent = '断开';
    off.addEventListener('click', async () => {
      try { await invoke('uninstall_hooks'); $('claude-err').style.display = 'none'; }
      catch (e) { $('claude-err').textContent = String(e); $('claude-err').style.display = 'block'; }
      renderConnection();
    });
    action.appendChild(off);
  } else {
    const btn = document.createElement('span');
    btn.className = 'btn dark';
    btn.textContent = '一键连接';
    btn.addEventListener('click', async () => {
      try { await invoke('install_hooks'); $('claude-err').style.display = 'none'; }
      catch (e) { $('claude-err').textContent = String(e); $('claude-err').style.display = 'block'; }
      renderConnection();
    });
    action.appendChild(btn);
  }
}

// ---------- 表单回填 ----------

function fillForm() {
  document.querySelectorAll('.tg[data-cfg]').forEach((el) =>
    el.classList.toggle('on', !!cfg[el.dataset.cfg]));
  document.querySelectorAll('.step[data-cfg]').forEach((el) => {
    el.querySelector('.val').textContent = `${cfg[el.dataset.cfg]} 分钟`;
  });
  $('size-slider')._paint?.(cfg.size);
}

// ---------- 启动 ----------

async function boot() {
  cfg = await invoke('get_config');
  packs = await invoke('get_packs');
  bindSlider();
  bindToggles();
  bindSteppers();
  fillForm();
  renderPacks();
  renderConnection();

  await listen('config-changed', async (e) => {
    cfg = e.payload;
    fillForm();
    renderPacks();
  });
  // 侧栏底部的状态词跟着桌面小猫走
  await listen('state-changed', (e) => {
    $('cur-state').textContent = STATE_WORDS[e.payload.state] ?? '待机中';
    renderConnection();
  });
  await listen('goto-page', (e) => gotoPage(e.payload));
}

boot();
