// Balance owed walkthrough (Addendum §16), client side: drives the real portal pages in headless
// Edge (DevTools protocol) against a stand-in for Stripe.
//   node client.mjs <base> <outDir> <apiLog> <email> <phase: pay> <stripe stand-in>
// The invitation link is read from the portal's log (EMAIL_PROVIDER=console), as a client reads
// their email. Writes PNGs and a JSON step log to outDir. Synthetic data only.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [BASE, OUT, LOG, EMAIL, PHASE, STRIPE] = process.argv.slice(2);
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
// VF_MOBILE=1: a phone (390x844, touch), and every screenshot also records what is wider than the screen.
const MOBILE = process.env.VF_MOBILE === '1';
const W = MOBILE ? 390 : 1280;
const H = MOBILE ? 844 : 900;
const DPORT = 9345;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const steps = [];
const note = (s) => {
  steps.push(s);
  console.log(s);
};

const profile = await mkdtemp(join(tmpdir(), 'edge-walk-'));
const edge = spawn(
  EDGE,
  ['--headless=new', `--remote-debugging-port=${DPORT}`, `--user-data-dir=${profile}`, '--hide-scrollbars', '--no-first-run', `--window-size=${W},${H}`, 'about:blank'],
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
await send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: MOBILE ? 2 : 1, mobile: MOBILE });
if (MOBILE) await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
const overflow = [];

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
  if (MOBILE) {
    const wide = await js(`(() => {
      if (!location.port || location.port === '3398') return null;
      const vw = document.documentElement.clientWidth;
      const out = [];
      if (document.documentElement.scrollWidth > vw + 1) out.push('page scrolls sideways: ' + document.documentElement.scrollWidth + 'px');
      for (const e of document.querySelectorAll('body *')) {
        const r = e.getBoundingClientRect();
        if (r.width === 0 || getComputedStyle(e).visibility === 'hidden') continue;
        let clipped = false;
        for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
          const o = getComputedStyle(p).overflowX;
          if (o === 'auto' || o === 'scroll' || o === 'hidden') { clipped = true; break; }
        }
        const label = (e.innerText || e.value || e.getAttribute('aria-label') || '').trim().slice(0, 40).split(String.fromCharCode(10)).join(' ');
        if (!clipped && (r.right > vw + 1 || r.left < -1))
          out.push('wide ' + e.tagName.toLowerCase() + ' ' + Math.round(r.left) + '..' + Math.round(r.right) + ' "' + label + '"');
        if (['BUTTON', 'A', 'INPUT', 'SELECT'].includes(e.tagName) && r.height > 0 && r.height < 32 && !['checkbox', 'radio', 'hidden'].includes(e.type))
          out.push('small target ' + Math.round(r.width) + 'x' + Math.round(r.height) + ' ' + e.tagName.toLowerCase() + ' "' + label + '"');
      }
      return out.slice(0, 40);
    })()`);
    if (wide && wide.length) overflow.push({ shot: name, url: await js('location.pathname'), issues: wide });
  }
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
  if (PHASE === 'pay') {
    // The client accepts the firm's portal invitation and lands on the home page.
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    note('Invitation accepted: the client is signed in');
    // The balance card, and Pay.
    await until(`document.body.innerText.includes('Balance due')`, 'the balance card');
    await sleep(800);
    await shot('10-client-balance-due');
    const text = await js(`document.body.innerText`);
    note(`Client card: ${text.includes('$240.00') ? '$240.00' : 'no amount'}; ${text.includes('2025 bookkeeping') ? 'memo shown' : 'no memo'}; ${text.includes('Available here until') ? 'date shown' : 'no date'}`);
    // Pay goes to Stripe's page (the stand-in's address cannot be opened: the session it made
    // is read from the stand-in instead, as Stripe would show its page).
    // Both methods on: a button each; the client pays by card.
    const both = await js(`!!__byText('button', 'Pay by bank account $240.00')`);
    note(`Pay buttons: ${both ? 'bank account and card' : 'one'}`);
    await js(`(() => { const b = [...document.querySelectorAll('button')].find((e) => e.textContent.trim().startsWith('Pay by card')); b.click(); return true; })()`);
    let session = null;
    for (let i = 0; i < 40 && !session; i++) {
      await sleep(250);
      session = await (await fetch(`${STRIPE}/_last`)).json();
    }
    if (!session) throw new Error('no Checkout Session was made');
    note(`Pay made a Checkout Session for balance ${session.metadata.balance_id} (${session.metadata.method})`);
    // The client pays on Stripe's page: the stand-in completes it and signs the webhook.
    const paid = await (
      await fetch(`${STRIPE}/_pay`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId: session.id, balanceId: session.metadata.balance_id, paid: true }),
      })
    ).json();
    note(`Stripe's webhook answered ${paid.webhook}`);
    // Back from Stripe's page.
    await go(`${BASE}/?balance=${session.metadata.balance_id}&session_id=${session.id}`);
    await until(`document.body.innerText.includes('Paid')`, 'the card says Paid');
    await sleep(800);
    await shot('11-client-balance-paid');
    const receipt = await js(`(async () => { const a = [...document.querySelectorAll('a')].find((e) => e.textContent.trim() === 'Receipt'); if (!a) return 0; const r = await fetch(a.href); const b = new Uint8Array(await r.arrayBuffer()); return r.ok && String.fromCharCode(...b.slice(0, 4)) === '%PDF' ? b.length : -1; })()`);
    note(`Paid card shows a Receipt link; the receipt is a PDF of ${receipt} bytes`);
    if (receipt <= 0) throw new Error('no receipt PDF');
  }
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}.json`), JSON.stringify(steps, null, 2));
  if (MOBILE) await writeFile(join(OUT, `mobile-${PHASE}.json`), JSON.stringify(overflow, null, 2));
  ws.close();
  edge.kill();
}
