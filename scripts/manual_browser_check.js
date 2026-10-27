// The built manual in a real browser (dev plan 27.1-27.2): drives a headless Edge or Chrome
// over the DevTools protocol through the manual opened from disk (file://), as a user would.
//
//   node scripts/manual_browser_check.js <browser.exe> <name> <manual folder> <output folder>
//
// It checks the manual's structure, not its words, so new pages keep it passing:
// - the search index and manual.js load as classic scripts from file://; the footer names the
//   index's version; `/` focuses the search box; a word from a page title finds that page, with
//   the words marked; the arrows move the selection; Enter opens the result with the words
//   highlighted on the page (`?q=`) and Clear removes them;
// - every figure on every page: its image loads at the size the generator planned
//   (`data-size`), the dim mask matches it, and each highlight lands on its mark's rect in the
//   image (`data-rect`, after crop and scale) within 3 image pixels, its numbered callout
//   listed under the picture; each badge clear of every outline (its own included) and of
//   the other badges, inside the figure's frame; each figure is saved as
//   <name>-figure-<page>-<n>.png;
// - previous/next link a page's neighbours in the sidebar; dark mode follows the system; at
//   420 px the sidebar folds behind the menu button with no sideways scrolling; print hides
//   the navigation; no script errors anywhere.
// Prints `ok`/`FAIL:` lines and `RESULT: PASS|FAIL <name>`; exits 1 on a failure. Node 22+
// (fetch and WebSocket are built in).
'use strict';
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');

const [exe, name, manualDir, outDir] = process.argv.slice(2);
if (!exe || !name || !manualDir || !outDir) {
  console.log('FAIL: usage: node manual_browser_check.js <browser.exe> <name> <manual folder> <output folder>');
  process.exit(1);
}
const root = pathToFileURL(path.resolve(manualDir)).href.replace(/\/?$/, '/');
fs.mkdirSync(outDir, { recursive: true });
const port = 9350 + (name.length * 7 + name.charCodeAt(0)) % 40;
const profile = path.resolve(outDir, 'profile-' + name);
const failures = [];
const check = (ok, what) => {
  console.log((ok ? 'ok   ' : 'FAIL: ') + what);
  if (!ok) failures.push(what);
};
const sleep = ms => new Promise(r => setTimeout(r, ms));

(async () => {
  const browser = spawn(exe, ['--headless=new', '--remote-debugging-port=' + port,
    '--user-data-dir=' + profile, '--window-size=1400,1000', '--no-first-run',
    '--no-default-browser-check', '--hide-scrollbars', 'about:blank'], { stdio: 'ignore' });
  let target;
  for (let i = 0; i < 75 && !target; i++) {
    await sleep(200);
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      target = list.find(t => t.type === 'page');
    } catch (e) { }
  }
  if (!target) { console.log('FAIL: the browser never answered on port ' + port); browser.kill(); process.exit(1); }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise(r => ws.addEventListener('open', r));
  let id = 0; const pending = new Map(); const errors = []; let loaded = null;
  ws.addEventListener('message', ev => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
    if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
    if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') errors.push(m.params.entry.text + ' ' + (m.params.entry.url || ''));
    if (m.method === 'Page.loadEventFired' && loaded) { loaded(); loaded = null; }
  });
  const send = (method, params = {}) => new Promise(r => { const n = ++id; pending.set(n, r); ws.send(JSON.stringify({ id: n, method, params })); });
  const evaluate = async expr => {
    const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.result.exceptionDetails) throw new Error(expr + ': ' + JSON.stringify(r.result.exceptionDetails));
    return r.result.result.value;
  };
  const go = async url => { const p = new Promise(r => loaded = r); await send('Page.navigate', { url }); await p; await sleep(150); };
  const key = async (key, code, keyCode, text) => {
    await send('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: keyCode, text });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: keyCode });
  };
  const shot = async (file, clip) => {
    const r = await send('Page.captureScreenshot', clip ? { format: 'png', clip: { ...clip, scale: 1 } } : { format: 'png' });
    fs.writeFileSync(path.join(outDir, `${name}-${file}.png`), Buffer.from(r.result.data, 'base64'));
  };
  await send('Page.enable'); await send('Runtime.enable'); await send('Log.enable');
  await send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });

  // The index: the scripts, the sidebar, the footer.
  await go(root + 'index.html');
  check(await evaluate('typeof window.OXIDE_SEARCH === "object" && window.OXIDE_SEARCH.entries.length > 0'), 'search-index.js loaded from file://');
  check(await evaluate('typeof window.OxideManual === "object" && typeof window.OxideManual.search === "function"'), 'manual.js loaded');
  const pages = await evaluate('[...document.querySelectorAll(".ox-sidebar a")].map(a => ({ href: a.getAttribute("href"), title: a.textContent.trim() }))');
  check(pages.length >= 2 && pages[0].href === 'index.html', `the sidebar lists ${pages.length} pages, the index first`);
  check(await evaluate('!!document.querySelector(".ox-sidebar a[aria-current=page]")'), 'the sidebar marks the current page');
  check(await evaluate('document.querySelector(".ox-footer").textContent.includes(OXIDE_SEARCH.version)'), 'the footer names the version the index was built for');

  // Search: a word of the second page's title finds that page.
  const page = pages[1];
  const word = (page.title.match(/[A-Za-z]{4,}/g) || [page.title])[0].toLowerCase();
  await key('/', 'Slash', 191, '/');
  check(await evaluate('document.activeElement.id') === 'ox-search', '/ focuses the search box');
  await send('Input.insertText', { text: word });
  await sleep(200);
  const results = await evaluate('[...document.querySelectorAll("#ox-results li[role=option]")].map(li => ({ snip: li.querySelector(".ox-r-snip").innerHTML, where: li.querySelector(".ox-r-where").textContent, href: li.querySelector("a").getAttribute("href"), selected: li.getAttribute("aria-selected") }))');
  console.log(`     "${word}": ${results.length} results; first ${results[0] ? results[0].where + ' -> ' + results[0].href : 'none'}`);
  check(results.length > 0, `searching "${word}" shows results`);
  check(results.some(r => r.href.split('?')[0] === page.href.split('#')[0]), `a result opens "${page.title}"`);
  check(results.length > 0 && (results[0].snip.includes('<mark>') || results[0].where.length > 0), 'the first result shows where it is, with the words marked');
  check(results.length > 0 && results[0].selected === 'true', 'the first result is selected');
  await shot('1-search');
  if (results.length > 1) {
    await key('ArrowDown', 'ArrowDown', 40);
    check(await evaluate('document.querySelector("#ox-results li[aria-selected=true]").id') === 'ox-r-1', 'ArrowDown moves the selection');
    await key('ArrowUp', 'ArrowUp', 38);
    check(await evaluate('document.querySelector("#ox-results li[aria-selected=true]").id') === 'ox-r-0', 'ArrowUp moves it back');
  }
  {
    const p = new Promise(r => loaded = r); await key('Enter', 'Enter', 13, '\r'); await p; await sleep(250);
  }
  const opened = await evaluate('({ href: location.href, marks: document.querySelectorAll("mark.ox-hit").length, bar: !!document.querySelector(".ox-hits"), input: document.getElementById("ox-search").value })');
  console.log(`     opened ${opened.href} with ${opened.marks} highlighted words`);
  check(opened.href.includes('?q='), 'Enter opened the result with the question (?q=)');
  check(opened.marks > 0 && opened.bar, 'the opened page highlights the words, with its bar');
  check(opened.input === word, 'the search box keeps the words');
  await shot('2-highlighted');
  await evaluate('document.querySelector(".ox-hits button").click()');
  check(await evaluate('document.querySelectorAll("mark.ox-hit").length === 0 && !document.querySelector(".ox-hits")'), 'Clear removes the highlights and the bar');

  // Every page: its figures, and previous/next.
  let figures = 0, highlights = 0, badges = 0;
  for (let n = 0; n < pages.length; n++) {
    const p = pages[n];
    const file = p.href.split('#')[0];
    await go(root + file);
    const nav = await evaluate('({ prev: document.querySelector(".ox-prev")?.getAttribute("href") ?? null, next: document.querySelector(".ox-next")?.getAttribute("href") ?? null })');
    const prev = n > 0 ? pages[n - 1].href.split('#')[0] : null;
    const next = n + 1 < pages.length ? pages[n + 1].href.split('#')[0] : null;
    check(nav.prev === prev && nav.next === next, `${file}: previous ${prev}, next ${next}`);
    // Lazy images: bring each figure into view so its image loads.
    const count = await evaluate('document.querySelectorAll(".ox-figure").length');
    for (let f = 0; f < count; f++) {
      await evaluate(`document.querySelectorAll('.ox-figure')[${f}].scrollIntoView({ block: 'center' })`);
      await evaluate(`new Promise(r => { const img = document.querySelectorAll('.ox-figure')[${f}].querySelector('img'); if (!img || img.complete) r(); else { img.onload = r; img.onerror = r; setTimeout(r, 3000); } })`);
      const fig = await evaluate(`(() => {
        const f = document.querySelectorAll('.ox-figure')[${f}];
        const shot = f.querySelector('.ox-shot'); const img = f.querySelector('img');
        if (!img) return { missing: true };
        const box = img.getBoundingClientRect();
        const size = (shot.dataset.size || '').split(' ').map(Number);
        const mask = f.querySelector('svg.ox-dim');
        const frame = (f.querySelector('.ox-frame') || shot).getBoundingClientRect();
        const box4 = r => [r.left, r.top, r.right, r.bottom];
        return {
          frame: box4(frame),
          badges: [...f.querySelectorAll('.ox-hl')].map(h => { const c = h.querySelector(':scope > .ox-callout');
            return c ? { mark: h.dataset.mark, label: c.textContent, side: c.className.replace('ox-callout', '').trim(),
              box: box4(c.getBoundingClientRect()), hl: box4(h.getBoundingClientRect()) } : null; }).filter(Boolean),
          others: [...f.querySelectorAll('.ox-hl')].map(h => ({ mark: h.dataset.mark, hl: box4(h.getBoundingClientRect()) })),
          src: img.getAttribute('src'), loaded: img.complete && img.naturalWidth > 0,
          natural: [img.naturalWidth, img.naturalHeight], size,
          box: [box.width, box.height],
          mask: mask ? mask.getAttribute('viewBox') : null,
          callouts: [...f.querySelectorAll('.ox-callouts li .ox-callout')].map(c => c.textContent),
          hls: [...f.querySelectorAll('.ox-hl')].map(h => { const r = h.getBoundingClientRect();
            const kx = img.naturalWidth / box.width, ky = img.naturalHeight / box.height;
            return { mark: h.dataset.mark, want: (h.dataset.rect || '').split(' ').map(Number),
              got: [(r.left - box.left) * kx, (r.top - box.top) * ky, r.width * kx, r.height * ky],
              label: (h.querySelector('.ox-callout') || {}).textContent || null }; }),
        };
      })()`);
      const where = `${file} figure ${f + 1}`;
      if (fig.missing) { check(false, `${where}: has no image (a placeholder: the shot is missing)`); continue; }
      figures++;
      check(fig.loaded, `${where}: ${fig.src} loads`);
      check(fig.size.length === 2 && fig.natural[0] === fig.size[0] && fig.natural[1] === fig.size[1], `${where}: the image is ${fig.natural.join('x')}, as planned (${fig.size.join('x')})`);
      check(Math.abs(fig.box[0] / fig.box[1] - fig.natural[0] / fig.natural[1]) < 0.02, `${where}: shown undistorted (${fig.box.map(Math.round).join('x')} px)`);
      if (fig.mask) check(fig.mask === `0 0 ${fig.size[0]} ${fig.size[1]}`, `${where}: the dim mask covers the image (${fig.mask})`);
      for (const h of fig.hls) {
        highlights++;
        const off = Math.max(...h.want.map((v, i) => Math.abs(v - h.got[i])));
        const inside = h.want[0] >= 0 && h.want[1] >= 0 && h.want[0] + h.want[2] <= fig.size[0] + 0.01 && h.want[1] + h.want[3] <= fig.size[1] + 0.01;
        check(off < 3 && inside, `${where}: ${h.mark} at ${h.got.map(Math.round).join(', ')} on its mark ${h.want.map(Math.round).join(', ')} (${off.toFixed(2)} px off)`);
        if (h.label) check(fig.callouts.includes(h.label), `${where}: callout ${h.label} is listed under the picture`);
      }
      // Badges beside their outlines: never on their own, another outline or another badge,
      // and inside the figure's frame (the picture and any margin made for them).
      const overlap = (a, b) => Math.min(a[2], b[2]) - Math.max(a[0], b[0]) > 0.5 && Math.min(a[3], b[3]) - Math.max(a[1], b[1]) > 0.5;
      for (const b of fig.badges) {
        badges++;
        const onOutline = fig.others.filter(o => overlap(b.box, o.hl)).map(o => o.mark);
        const onBadge = fig.badges.filter(o => o !== b && overlap(b.box, o.box)).map(o => o.label);
        const inside = b.box[0] >= fig.frame[0] - 0.5 && b.box[1] >= fig.frame[1] - 0.5 && b.box[2] <= fig.frame[2] + 0.5 && b.box[3] <= fig.frame[3] + 0.5;
        check(!onOutline.length && !onBadge.length && inside,
          `${where}: badge ${b.label} (${b.side}) clear of ${b.mark}'s outline${onOutline.length ? ' — on ' + onOutline.join(', ') : ''}${onBadge.length ? ', on badge ' + onBadge.join(', ') : ''}${inside ? '' : ', outside the figure'}`);
      }
      const r = await evaluate(`(() => { const b = document.querySelectorAll('.ox-figure')[${f}].getBoundingClientRect(); return { x: b.left + scrollX, y: b.top + scrollY, width: b.width, height: b.height }; })()`);
      await shot(`figure-${file.replace(/\.html$/, '')}-${f + 1}`, r);
    }
  }
  console.log(`     ${figures} figures, ${highlights} highlights, ${badges} badges on ${pages.length} pages`);

  // Dark mode follows the system.
  const sample = pages[1].href.split('#')[0];
  await go(root + sample);
  const light = await evaluate('getComputedStyle(document.body).backgroundColor');
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
  await go(root + sample);
  const dark = await evaluate('getComputedStyle(document.body).backgroundColor');
  check(dark !== light, `dark mode: the background changes (${light} -> ${dark})`);
  await shot('3-dark');
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });

  // Narrow: the sidebar folds behind the menu button.
  await send('Emulation.setDeviceMetricsOverride', { width: 420, height: 860, deviceScaleFactor: 1, mobile: true });
  await go(root + sample);
  check(await evaluate('getComputedStyle(document.querySelector(".ox-sidebar")).display') === 'none', 'narrow: the sidebar is folded away');
  check(await evaluate('document.documentElement.scrollWidth <= 420'), 'narrow: no sideways scrolling');
  await evaluate('document.querySelector(".ox-menu").click()');
  await sleep(100);
  check(await evaluate('getComputedStyle(document.querySelector(".ox-sidebar")).display') !== 'none', 'narrow: the menu button shows the chapters');
  await shot('4-narrow-menu');
  await send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 1000, deviceScaleFactor: 1, mobile: false });

  // Print hides the navigation.
  await send('Emulation.setEmulatedMedia', { media: 'print' });
  check(await evaluate('getComputedStyle(document.querySelector(".ox-header")).display === "none" && getComputedStyle(document.querySelector(".ox-sidebar")).display === "none"'), 'print: header and sidebar hidden');
  await send('Emulation.setEmulatedMedia', { media: '' });

  check(errors.length === 0, 'no script errors' + (errors.length ? ': ' + errors.join(' | ') : ''));
  ws.close(); browser.kill();
  console.log(failures.length ? `RESULT: FAIL ${name}: ${failures.length} failed` : `RESULT: PASS ${name}: ${figures} figures, ${highlights} highlights on their marks, ${badges} badges clear of them`);
  process.exit(failures.length ? 1 : 0);
})().catch(e => { console.log('FAIL: ' + e.stack); process.exit(1); });
