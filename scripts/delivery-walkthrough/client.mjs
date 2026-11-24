// Tax return delivery walkthrough (Addendum §12, slice 2), client side: drives the real portal
// pages in headless Edge (DevTools protocol).
//   node client.mjs <base> <outDir> <apiLog> <email> <phase: deliver>
// The invitation link is read from the portal's log (EMAIL_PROVIDER=console), as a client reads
// their email. Writes PNGs and a JSON step log to outDir. Synthetic data only.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [BASE, OUT, LOG, EMAIL, PHASE] = process.argv.slice(2);
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const DPORT = 9346;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const steps = [];
const note = (s) => {
  steps.push(s);
  console.log(s);
};

const profile = await mkdtemp(join(tmpdir(), 'edge-walk-'));
const edge = spawn(
  EDGE,
  ['--headless=new', `--remote-debugging-port=${DPORT}`, `--user-data-dir=${profile}`, '--hide-scrollbars', '--no-first-run', '--window-size=1280,900', 'about:blank'],
  { stdio: 'ignore' },
);
let target;
for (let i = 0; i < 60 && !target; i++) {
  await sleep(250);
  try {
    target = (await (await fetch(`http://127.0.0.1:${DPORT}/json/list`)).json()).find((t) => t.type === 'page');
  } catch {}
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0;
const pending = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  }
});
const send = (method, params = {}) =>
  new Promise((r) => {
    const n = ++id;
    pending.set(n, r);
    ws.send(JSON.stringify({ id: n, method, params }));
  });
const js = async (expr) => {
  const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
  return r.result?.result?.value;
};
await send('Page.enable');
await send('DOM.enable');
await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });

// Helpers that run in the page.
await send('Page.addScriptToEvaluateOnNewDocument', {
  source: `
    window.__byText = (sel, text, root = document) =>
      [...root.querySelectorAll(sel)].find((e) => e.textContent.trim() === text && !e.disabled);
    window.__type = (el, value) => {
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
  `,
});

const go = async (url) => {
  await send('Page.navigate', { url });
  await sleep(1500);
};
const until = async (expr, what, ms = 15000) => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (await js(expr)) return;
    await sleep(250);
  }
  await shot(`FAILED-${what.replace(/\W+/g, '-')}`);
  throw new Error(`timed out: ${what}`);
};
const shot = async (name) => {
  const { result } = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
  await writeFile(join(OUT, `${name}.png`), Buffer.from(result.data, 'base64'));
};
const click = async (text, sel = 'button') => {
  const ok = await js(`(() => { const b = __byText(${JSON.stringify(sel)}, ${JSON.stringify(text)}); if (!b) return false; b.click(); return true; })()`);
  if (!ok) throw new Error(`no ${sel} "${text}"`);
  await sleep(600);
};
/** The newest message to EMAIL in the portal's log whose text has `has`. */
const mailWith = async (has, after) => {
  for (let i = 0; i < 80; i++) {
    const lines = (await readFile(LOG, 'utf8')).split('\n').filter((l) => l.includes('"msg":"notify ('));
    const hits = lines
      .map((l) => JSON.parse(l))
      .filter((m) => m.to === EMAIL && m.body?.includes(has) && m.time > after);
    if (hits.length) return hits.at(-1);
    await sleep(250);
  }
  throw new Error(`no email to ${EMAIL} with ${has}`);
};
const linkIn = (body, part) => body.match(new RegExp(`https?://\\S*${part}\\S*`))[0];

try {
  if (PHASE === 'deliver') {
    // The client accepts the firm's portal invitation and lands on the home page.
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    note('Invitation accepted: the client is signed in');
    await until(`document.body.innerText.includes('Your 2025 tax return')`, 'the tax return card');
    await sleep(800);
    await shot('10-client-card');
    const text = await js(`document.body.innerText`);
    note(`Card: ${(text.match(/\d of \d done/) || ['?'])[0]}; ${text.includes('Review your tax return') ? 'return step' : 'no return step'}; ${text.includes('Form 8879') ? '8879 step' : 'no 8879 step'}`);
    // View: the return opens in a new tab; the page's own fetch of it stands for that tab.
    const viewed = await js(`(async () => { const a = [...document.querySelectorAll('a')].find((e) => e.textContent.trim() === 'View'); if (!a) return 0; const r = await fetch(a.href); return r.status; })()`);
    note(`View answered ${viewed}`);
    // Open eSignature (logged), then I've signed it.
    await js(`(() => { const a = [...document.querySelectorAll('a')].find((e) => e.textContent.trim() === 'Open eSignature'); a.removeAttribute('target'); a.addEventListener('click', (e) => e.preventDefault()); a.click(); return true; })()`);
    await sleep(600);
    await go(`${BASE}/`);
    await until(`!!__byText('button', "I've signed it")`, 'the 8879 step');
    await click("I've signed it");
    await until(`document.body.innerText.includes('Waiting for the firm to confirm')`, 'waiting');
    await sleep(600);
    await shot('11-client-waiting');
    note('The client said they signed Form 8879: Waiting for the firm to confirm');
  }
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}.json`), JSON.stringify(steps, null, 2));
  ws.close();
  edge.kill();
}
