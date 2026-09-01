// DeskCat 组件行为 —— 纯原生,无框架。事件用 CustomEvent 冒泡,由页面决定怎么落配置。
// 约定:每个可交互组件带 data-key,变更时派发 dc:change {key, value}。

function emit(el, key, value) {
  el.dispatchEvent(new CustomEvent('dc:change', { bubbles: true, detail: { key, value } }));
}

/** Toggle:点击切换 .on */
export function bindToggles(root = document) {
  root.querySelectorAll('.tg[data-key]').forEach((el) => {
    el.addEventListener('click', () => {
      el.classList.toggle('on');
      emit(el, el.dataset.key, el.classList.contains('on'));
    });
  });
}

/** Slider:点击/拖动改 --fill(百分比);min/max 映射到实际值 */
export function bindSliders(root = document) {
  root.querySelectorAll('.slider[data-key]').forEach((el) => {
    const min = Number(el.dataset.min ?? 0);
    const max = Number(el.dataset.max ?? 100);
    const apply = (e) => {
      const r = el.getBoundingClientRect();
      const pct = Math.min(100, Math.max(0, ((e.clientX - r.left) / r.width) * 100));
      el.style.setProperty('--fill', `${pct}%`);
      emit(el, el.dataset.key, Math.round(min + (pct / 100) * (max - min)));
    };
    // 轨道只有 4px 高,指针极易滑出;监听挂在 document 上才拖得住
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
  });
}

/** Stepper:− / 值 / ＋;值域与步长由 data-min/max/step 定 */
export function bindSteppers(root = document) {
  root.querySelectorAll('.step[data-key]').forEach((el) => {
    const val = el.querySelector('.val');
    const min = Number(el.dataset.min ?? 1);
    const max = Number(el.dataset.max ?? 999);
    const stepBy = Number(el.dataset.step ?? 1);
    const unit = el.dataset.unit ?? '';
    const set = (n) => {
      const v = Math.min(max, Math.max(min, n));
      val.textContent = `${v} ${unit}`.trim();
      el.dataset.value = String(v);
      emit(el, el.dataset.key, v);
    };
    el.querySelectorAll('.pm').forEach((btn) => {
      btn.addEventListener('click', () => {
        set(Number(el.dataset.value ?? min) + (btn.dataset.dir === 'up' ? stepBy : -stepBy));
      });
    });
  });
}

/** NavItem:单选,切换 .sel 并派发页面切换 */
export function bindNav(root = document) {
  root.querySelectorAll('.nav[data-key]').forEach((nav) => {
    nav.querySelectorAll('.ni').forEach((ni) => {
      ni.addEventListener('click', () => {
        nav.querySelectorAll('.ni').forEach((o) => o.classList.remove('sel'));
        ni.classList.add('sel');
        emit(nav, nav.dataset.key, ni.dataset.page);
      });
    });
  });
}

export function bindAll(root = document) {
  bindToggles(root);
  bindSliders(root);
  bindSteppers(root);
  bindNav(root);
}
