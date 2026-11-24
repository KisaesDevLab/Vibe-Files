// Tax return delivery walkthrough (Addendum §12, slice 2), client side: drives the real portal
// pages in headless Edge (DevTools protocol).
//   node client.mjs <base> <outDir> <apiLog> <email> <phase: deliver|letters|fee> [second signer's email | stripe stand-in]
// The invitation link is read from the portal's log (EMAIL_PROVIDER=console), as a client reads
// their email. Writes PNGs and a JSON step log to outDir. Synthetic data only.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [BASE, OUT, LOG, EMAIL, PHASE, SECOND] = process.argv.slice(2);
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


/** The newest message to `to` in the portal's log whose text has `has`. */
const mailTo = async (to, has) => {
  for (let i = 0; i < 80; i++) {
    const lines = (await readFile(LOG, 'utf8')).split('\n').filter((l) => l.includes('"msg":"notify ('));
    const hits = lines.map((l) => JSON.parse(l)).filter((m) => m.to === to && m.body?.includes(has));
    if (hits.length) return hits.at(-1);
    await sleep(250);
  }
  throw new Error(`no email to ${to} with ${has}`);
};
/** Draws a signature on the pad with pointer events, as a finger or mouse does. */
const draw = async () => {
  await js(`(() => {
    const c = document.querySelector('canvas[aria-label="Draw your signature"]');
    const r = c.getBoundingClientRect();
    const ev = (type, x, y) => c.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1, clientX: r.left + x, clientY: r.top + y }));
    ev('pointerdown', 20, 80);
    for (let i = 1; i <= 30; i++) ev('pointermove', 20 + i * 12, 80 + Math.sin(i / 3) * 30);
    ev('pointerup', 380, 80);
    return true;
  })()`);
};

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
  } else if (PHASE === 'letters') {
    // Jordan accepts the invitation, then signs; Casey signs next on the same device.
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`!!__byText('a', 'Sign')`, 'the letters step');
    await shot('20-client-card-letters');
    await js(`__byText('a', 'Sign').click()`);
    await until(`location.pathname.endsWith('/sign')`, 'the sign page');
    for (let round = 0; round < 6; round++) {
      await until(`document.body.innerText.includes('All letters signed') || !!document.querySelector('canvas')`, 'a letter to sign', 20000);
      if (await js(`document.body.innerText.includes('All letters signed')`)) break;
      const turn = await js(`[...document.querySelectorAll('strong')].map((e) => e.textContent).find((t) => t.startsWith('Sign as') || t.includes('your turn')) || ''`);
      if (turn.includes('your turn') && (await js(`!!__byText('button', 'Send the code')`))) {
        await click('Send the code');
        await until(`document.body.innerText.includes('We sent a code')`, 'code sent');
        const m = await mailTo(SECOND, 'your code to sign');
        const code = /is (\d{6})/.exec(m.body)[1];
        note(`${turn}: the code reached ${SECOND}`);
        await js(`__type([...document.querySelectorAll('input')].find((i) => i.autocomplete === 'one-time-code'), ${JSON.stringify(code)})`);
        await click('Check the code');
        await until(`document.body.innerText.includes('Code accepted')`, 'code accepted');
      }
      await draw();
      await js(`(() => { const c = [...document.querySelectorAll('input[type=checkbox]')].find((i) => i.parentElement.textContent.includes('agree to sign')); c.click(); return true; })()`);
      await sleep(300);
      await shot(`21-sign-round-${round}`);
      const btn = await js(`[...document.querySelectorAll('button')].map((b) => b.textContent.trim()).find((t) => t.startsWith('Sign as'))`);
      await click(btn);
      const signedBy = `Signed by ${btn.replace('Sign as ', '')}.`;
      await until(`document.body.innerText.includes(${JSON.stringify(signedBy)}) || document.body.innerText.includes('All letters signed')`, 'signed');
      note(`${btn}: signed`);
      await sleep(1500);
    }
    await until(`document.body.innerText.includes('All letters signed')`, 'all letters signed');
    await shot('22-all-signed');
    note('All letters signed');
    // The signed copy in the client's files, as the portal's viewer shows it.
    const files = await js(`fetch('/api/portal/files').then((r) => r.json())`);
    const signed = files.files.find((f) => f.name.endsWith('- signed.pdf'));
    note(`Signed copy in the client's files: ${signed ? signed.name : 'none'}`);
    if (signed) {
      await go(`${BASE}/files/${signed.id}`);
      await sleep(3000);
      await shot('23-signed-copy');
    }
  } else if (PHASE === 'fee') {
    // The client chooses a package and an add-on, reads the addendum, signs, and pays by card.
    const STRIPE = SECOND;
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`!!__byText('a', 'Choose')`, 'the package step');
    await shot('30-client-card-package');
    await js(`__byText('a', 'Choose').click()`);
    await until(`location.pathname.endsWith('/package')`, 'the package page');
    await until(`document.body.innerText.includes('Advantage')`, 'packages listed');
    const none = await js(`[...document.querySelectorAll('input[type=radio]')].every((r) => !r.checked)`);
    note(`Nothing chosen at first: ${none}`);
    await js(`(() => { const l = [...document.querySelectorAll('label')].find((x) => x.textContent.includes('Advantage') && x.querySelector('input[type=radio]')); l.querySelector('input').click(); return true; })()`);
    await sleep(400);
    await js(`(() => { const l = [...document.querySelectorAll('label')].find((x) => x.textContent.includes('Mid-year planning meeting') && x.querySelector('input[type=checkbox]')); l.querySelector('input').click(); return true; })()`);
    await sleep(600);
    const total = await js(`(document.body.innerText.match(/Total due in the next step\\s*\\$([\\d,.]+)/) || [])[1] || ''`);
    note(`Advantage and the planning meeting: total $${total}`);
    // Reading the addendum (it opens in a new tab; the click marks it read).
    await js(`(() => { const a = [...document.querySelectorAll('a')].find((x) => x.textContent.startsWith('Read the')); a.removeAttribute('target'); a.addEventListener('click', (e) => e.preventDefault()); a.click(); return true; })()`);
    await draw();
    await js(`(() => { const c = [...document.querySelectorAll('input[type=checkbox]')].find((i) => i.parentElement.textContent.includes('I choose this selection')); c.click(); return true; })()`);
    await sleep(400);
    await shot('31-package-signed-ready');
    await click('Sign and continue to payment');
    await until(`location.pathname === '/'`, 'back to the card');
    await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim().startsWith('Pay by card'))`, 'the fee step');
    await shot('32-card-fee');
    await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim().startsWith('Pay by card')).click()`);
    let session = null;
    for (let i = 0; i < 40 && !session; i++) {
      await sleep(250);
      session = await (await fetch(`${STRIPE}/_last`)).json();
    }
    if (!session) throw new Error('no Checkout Session');
    const paid = await (
      await fetch(`${STRIPE}/_pay`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId: session.id, paid: true }),
      })
    ).json();
    note(`Paid by card on Stripe's page (the stand-in); webhook ${paid.webhook}`);
    await go(`${BASE}/?delivery=${session.metadata.delivery_id}&session_id=${session.id}`);
    await until(`!!__byText('a', 'Receipt')`, 'the receipt link');
    await shot('33-card-paid');
    note('The fee step shows its Receipt');
  }
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}.json`), JSON.stringify(steps, null, 2));
  ws.close();
  edge.kill();
}
