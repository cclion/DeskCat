/**
 * 渲染 HTML 截图。
 * 用法:
 *   node scripts/render-screen.cjs <文件> <选择器> <输出名>
 *   选择器: 留空 = 整页 fullPage; "tag:C.01" = 定位 screens.html 里某手机卡; 其它 = CSS 选择器
 * 例:
 *   node scripts/render-screen.cjs design/wireframe-home.html "" /tmp/wf
 *   node scripts/render-screen.cjs design/screens.html "tag:C.01" /tmp/c01
 * playwright 自动从 npx 缓存解析（仓库未装）。
 */
const path = require('path');
const { execSync } = require('child_process');

function loadChromium() {
  try { return require('playwright').chromium; } catch {}
  const dirs = execSync(
    `find ${process.env.HOME}/.npm/_npx -maxdepth 5 -type d -name playwright -path '*node_modules*' 2>/dev/null || true`
  ).toString().trim().split('\n').filter(Boolean);
  for (const d of dirs) { try { return require(d).chromium; } catch {} }
  throw new Error('playwright 未找到');
}

(async () => {
  const file = process.argv[2] || 'design/screens.html';
  const sel = process.argv[3] || '';
  const out = process.argv[4] || '/tmp/render';
  const chromium = loadChromium();

  const browser = await chromium.launch();
  const page = await browser.newPage({ deviceScaleFactor: 2 });
  await page.setViewportSize({ width: 1700, height: 1200 });
  await page.goto('file://' + path.resolve(file), { waitUntil: 'load' });

  if (!sel) {
    await page.screenshot({ path: out + '.png', fullPage: true });
  } else if (sel.startsWith('tag:')) {
    const ph = page.locator('.col', { hasText: sel.slice(4) }).locator('.phone').first();
    await ph.scrollIntoViewIfNeeded();
    await ph.screenshot({ path: out + '.png' });
  } else {
    await page.locator(sel).first().screenshot({ path: out + '.png' });
  }

  await browser.close();
  console.log('✓ ' + out + '.png');
})().catch((e) => { console.error('✗', e.message); process.exit(1); });
