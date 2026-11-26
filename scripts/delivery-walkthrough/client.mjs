// Tax return delivery walkthrough (Addendum §12, slice 2), client side: drives the real portal
// pages in headless Edge (DevTools protocol).
//   node client.mjs <base> <outDir> <apiLog> <email> <phase: deliver|letters|fee> [second signer's email | stripe stand-in]
// The invitation link is read from the portal's log (EMAIL_PROVIDER=console), as a client reads
// their email. Writes PNGs and a JSON step log to outDir. Synthetic data only.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { W, H, emulate, layoutLog } from '../walkthrough/mobile.mjs';

const [BASE, OUT, LOG, EMAIL, PHASE, SECOND, THIRD] = process.argv.slice(2);
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
await emulate(send);
const layout = layoutLog(js, writeFile);

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
  await layout.check(name);
  const { result } = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
  await writeFile(join(OUT, `${name}.png`), Buffer.from(result.data, 'base64'));
};
const click = async (text, sel = 'button') => {
  const ok = await js(`(() => { const b = __byText(${JSON.stringify(sel)}, ${JSON.stringify(text)}); if (!b) return false; b.click(); return true; })()`);
  if (!ok) {
    await shot(`FAILED-no-${text.replace(/\W+/g, '-')}`);
    throw new Error(`no ${sel} "${text}"`);
  }
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
    const ev = (type, x, y) => c.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1, clientX: r.left + (x * Math.min(r.width, 400)) / 400, clientY: r.top + y }));
    ev('pointerdown', 20, 80);
    for (let i = 1; i <= 30; i++) ev('pointermove', 20 + i * 12, 80 + Math.sin(i / 3) * 30);
    ev('pointerup', 380, 80);
    return true;
  })()`);
};

/** Signs every letter on the Sign page; the second signer (SECOND) by the code sent to them. */
const signAll = async (prefix) => {
  for (let round = 0; round < 10; round++) {
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
    if (round === 0) await shot(`${prefix}-sign`);
    const btn = await js(`[...document.querySelectorAll('button')].map((b) => b.textContent.trim()).find((t) => t.startsWith('Sign as'))`);
    await click(btn);
    note(`Signed: ${btn}`);
    // The page moves on (the next letter or signer) before the next round reads it.
    await until(`!__byText('button', ${JSON.stringify(btn)}) || document.body.innerText.includes('All letters signed') || document.body.innerText.includes('your turn')`, 'the next letter', 20000);
    await sleep(800);
  }
  await until(`document.body.innerText.includes('All letters signed')`, 'all letters signed', 20000);
  note('All letters signed');
};

/** Pays the step shown (the button starting with `label`) on the stand-in: paid, or started. */
const payOnStripe = async (STRIPE, label, paid) => {
  await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim().startsWith(${JSON.stringify(label)}))`, label);
  const before = await (await fetch(`${STRIPE}/_last`)).json();
  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim().startsWith(${JSON.stringify(label)})).click()`);
  let session = null;
  for (let i = 0; i < 40; i++) {
    await sleep(250);
    const s = await (await fetch(`${STRIPE}/_last`)).json();
    if (s && s.id !== before?.id) {
      session = s;
      break;
    }
  }
  if (!session) throw new Error('no Checkout Session');
  const r = await (
    await fetch(`${STRIPE}/_pay`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, paid }),
    })
  ).json();
  note(`${label}: Checkout in ${session.mode} mode${session.mode === 'subscription' ? `, monthly ${session.monthly} cents` : ''}; ${paid ? 'paid' : 'started (bank)'}; webhook ${r.webhook}`);
  await go(`${BASE}/?delivery=${session.metadata.delivery_id}&session_id=${session.id}`);
  return session;
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
  } else if (PHASE === 'agreement') {
    // Annual agreement (2026-11-25): the client signs the bookkeeping letter, then sets up
    // payment by card (the first month and the one-time fee); a month later a payment fails
    // and the card offers Update payment method.
    const STRIPE = SECOND;
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`document.body.innerText.includes('service agreement')`, 'the agreement card');
    await sleep(800);
    await shot('40-client-agreement-card');
    note(`Card: ${(await js(`document.body.innerText`)).includes('Set up payment') ? 'letters, then Set up payment' : '?'}`);
    await js(`__byText('a', 'Sign').click()`);
    await until(`location.pathname.endsWith('/sign')`, 'the sign page');
    for (let round = 0; round < 8; round++) {
      await until(`document.body.innerText.includes('All letters signed') || !!document.querySelector('canvas')`, 'a letter to sign', 20000);
      if (await js(`document.body.innerText.includes('All letters signed')`)) break;
      await draw();
      await js(`(() => { const c = [...document.querySelectorAll('input[type=checkbox]')].find((i) => i.parentElement.textContent.includes('agree to sign')); c.click(); return true; })()`);
      await sleep(300);
      if (round === 0) await shot('41-agreement-sign');
      const btn = await js(`[...document.querySelectorAll('button')].map((b) => b.textContent.trim()).find((t) => t.startsWith('Sign as'))`);
      await click(btn);
      note(`Letter ${round + 1}: ${btn}`);
      await sleep(1500);
    }
    await until(`document.body.innerText.includes('All letters signed')`, 'all letters signed', 20000);
    note('All letters signed');
    await go(`${BASE}/`);
    await until(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim().startsWith('Set up payment by card'))`, 'the payment step');
    await shot('42-agreement-set-up-payment');
    const today = await js(`(document.body.innerText.split(String.fromCharCode(10)).find((l) => l.startsWith('Today ')) || '')`);
    note(`The card says: ${today}`);
    await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim().startsWith('Set up payment by card')).click()`);
    let session = null;
    for (let i = 0; i < 40 && !session; i++) {
      await sleep(250);
      session = await (await fetch(`${STRIPE}/_last`)).json();
    }
    if (!session || session.mode !== 'subscription') throw new Error('no subscription Checkout Session');
    const paid = await (
      await fetch(`${STRIPE}/_pay`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId: session.id, paid: true }),
      })
    ).json();
    note(`Set up on Stripe's page (the stand-in, subscription mode, monthly ${session.monthly} cents); webhook ${paid.webhook}`);
    await go(`${BASE}/?delivery=${session.metadata.delivery_id}&session_id=${session.id}`);
    await until(`document.body.innerText.includes('Your monthly payment')`, 'the monthly payment');
    await shot('43-monthly-active');
    note(`Monthly payment: ${(await js(`document.body.innerText`)).includes('Active') ? 'Active' : '?'}`);
    const last = await (await fetch(`${STRIPE}/_last`)).json();
    const month = await (
      await fetch(`${STRIPE}/_month`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ subscription: last.subscription, paid: true }),
      })
    ).json();
    note(`A month paid: webhook ${month.webhook}`);
    const failed = await (
      await fetch(`${STRIPE}/_month`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ subscription: last.subscription, paid: false }),
      })
    ).json();
    note(`The next month failed: webhook ${failed.webhook}`);
    await go(`${BASE}/`);
    await until(`document.body.innerText.includes('did not go through')`, 'the failed month');
    await shot('44-monthly-failed');
    note('The card says the last payment did not go through, with Update payment method');
  } else if (PHASE === 'agreement-joint') {
    // A joint couple: Jordan accepts, both sign (Casey by a code), Set up payment by bank
    // account: Payment started, then the bank clears it. SECOND: Casey's email; THIRD: Stripe.
    const STRIPE = THIRD;
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`document.body.innerText.includes('service agreement')`, 'the agreement card');
    await shot('50-joint-card');
    await js(`__byText('a', 'Sign').click()`);
    await until(`location.pathname.endsWith('/sign')`, 'the sign page');
    await signAll('51-joint');
    await go(`${BASE}/`);
    const s = await payOnStripe(STRIPE, 'Set up payment by bank account', false);
    await until(`document.body.innerText.includes('Payment started')`, 'payment started');
    await shot('52-joint-bank-started');
    note('The card says Payment started (a bank payment takes days)');
    const r = await (
      await fetch(`${STRIPE}/_settle`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId: s.id }),
      })
    ).json();
    note(`The bank cleared it: webhook ${r.webhook}`);
    await go(`${BASE}/`);
    await until(`document.body.innerText.includes('Your monthly payment') && document.body.innerText.includes('by bank account')`, 'the monthly payment by bank');
    await shot('53-joint-monthly-active');
    note('Your monthly payment: by bank account, Active');
  } else if (PHASE === 'renew') {
    // Next year: Jordan signs in again (a sign-in link by email), both sign the renewal, and
    // only the one-time fee is paid; the monthly payment continues. SECOND: Casey; THIRD: Stripe.
    const STRIPE = THIRD;
    const start = Date.now() / 1000;
    await go(`${BASE}/login`);
    await until(`!!document.querySelector('input')`, 'the sign-in page');
    await js(`__type(document.querySelector('input'), ${JSON.stringify(EMAIL)})`);
    await click('Continue');
    const m = await mailWith('/auth/verify', start);
    await go(linkIn(m.body, '/auth/verify'));
    await until(`!!__byText('button', 'Sign in')`, 'the sign-in link');
    await click('Sign in');
    await until(`location.pathname === '/'`, 'home after signing in');
    note('Signed in again by an emailed link');
    await until(`!!__byText('a', 'Sign')`, 'the renewal letters');
    await shot('60-renew-card');
    await js(`__byText('a', 'Sign').click()`);
    await until(`location.pathname.endsWith('/sign')`, 'the sign page');
    await signAll('61-renew');
    await go(`${BASE}/`);
    const says = await js(`document.body.innerText.includes('Your monthly payment continues at the new amount')`);
    note(`The fee step says the monthly payment continues: ${says}`);
    await payOnStripe(STRIPE, 'Pay by card', true);
    await until(`!!__byText('a', 'Receipt')`, 'the receipt link');
    await shot('62-renew-paid');
    note("The renewal's one-time fee is paid; its Receipt is shown");
  } else if (PHASE === 'statements') {
    // PA-114: a financial statements card of two PDFs that open only once the fee is paid.
    // The client accepts the invitation and sees the documents' names, locked.
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`document.body.innerText.includes('financial statements')`, 'the statements card');
    await sleep(800);
    await shot('70-statements-locked');
    const text = await js(`document.body.innerText`);
    note(`Card says "Opens when you pay.": ${text.includes('Opens when you pay.')}`);
    note(`Locked names listed: ${(text.match(/[^\n]*\(locked\)/g) || []).join(' | ') || 'none'}`);
    note(`No View link yet: ${!(await js(`[...document.querySelectorAll('a')].some((a) => a.textContent.trim() === 'View')`))}`);
    // The files list, and the content of each asked for directly (a link guessed or kept).
    const files = await js(`fetch('/api/portal/files').then((r) => r.json())`);
    for (const f of files.files) {
      const st = await js(`fetch('/api/portal/files/${f.id}/content?mode=view').then((r) => r.status)`);
      note(`Files list: ${f.name}${f.held ? ' (held)' : ''}; its content answers ${st}`);
    }
  } else if (PHASE === 'statements-pay') {
    // Signs in again by an emailed link, pays by card, then opens both documents.
    const STRIPE = SECOND;
    const start = Date.now() / 1000;
    await go(`${BASE}/login`);
    await until(`!!document.querySelector('input')`, 'the sign-in page');
    await js(`__type(document.querySelector('input'), ${JSON.stringify(EMAIL)})`);
    await click('Continue');
    const m = await mailWith('/auth/verify', start);
    await go(linkIn(m.body, '/auth/verify'));
    await until(`!!__byText('button', 'Sign in')`, 'the sign-in link');
    await click('Sign in');
    await until(`location.pathname === '/'`, 'home after signing in');
    await payOnStripe(STRIPE, 'Pay by card', true);
    await until(`[...document.querySelectorAll('a')].some((a) => a.textContent.trim() === 'View')`, 'the documents open', 20000);
    await shot('71-statements-paid-open');
    const views = await js(`[...document.querySelectorAll('a')].filter((a) => (a.getAttribute('aria-label') || '').startsWith('View ')).map((a) => [a.getAttribute('aria-label'), a.href])`);
    note(`After paying: ${views.length} View links`);
    for (const [label, href] of views) {
      const st = await js(`fetch(${JSON.stringify(href)}).then((r) => r.status)`);
      note(`${label}: ${st}`);
      if (views.length > 1 && label === views[0][0]) {
        await go(`${BASE}/`);
        await until(`document.body.innerText.includes('Opened')`, 'one opened');
        await shot('72-statements-one-opened');
        note(`After the first: step still open: ${!(await js(`document.body.innerText.includes('2 of 2 done')`))}; ${(await js(`document.body.innerText`)).match(/\d of \d done/)?.[0] ?? '?'}`);
      }
    }
    await go(`${BASE}/`);
    await until(`(document.body.innerText.match(/Opened/g) || []).length >= 2 || document.body.innerText.includes('All done') || document.body.innerText.includes('Complete')`, 'both opened', 20000);
    await sleep(600);
    await shot('73-statements-both-opened');
    note(`After both: ${(await js(`document.body.innerText`)).match(/\d of \d done/)?.[0] ?? 'no count shown'}`);
  } else if (PHASE === 'revise-pay') {
    // Owner 2026-11-26: the client views the return and pays the fee by card.
    const STRIPE = SECOND;
    const inv = await mailWith('/invite', 0);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    await until(`document.body.innerText.includes('Your 2025 tax return')`, 'the tax return card');
    await sleep(800);
    await shot('90-revise-card');
    const viewed = await js(`(async () => { const a = [...document.querySelectorAll('a')].find((e) => e.textContent.trim() === 'View'); if (!a) return 0; const r = await fetch(a.href); return r.status; })()`);
    note(`View answered ${viewed}`);
    if (viewed !== 200) throw new Error(`View answered ${viewed}`);
    await go(`${BASE}/`);
    await payOnStripe(STRIPE, 'Pay by card', true);
    await until(`!!__byText('a', 'Receipt')`, 'the receipt link');
    await sleep(600);
    await shot('91-revise-paid');
    const mine = await js(`fetch('/api/portal/deliveries').then((r) => r.json())`);
    const d = mine.deliveries[0];
    note(`Paid: steps ${d.steps.map((s) => `${s.kind} ${s.state}`).join(', ')}`);
  } else if (PHASE === 'revise-view') {
    // Signs in again: the card says Updated, the fee is still paid, the corrected return opens.
    const start = Date.now() / 1000;
    await go(`${BASE}/login`);
    await until(`!!document.querySelector('input')`, 'the sign-in page');
    await js(`__type(document.querySelector('input'), ${JSON.stringify(EMAIL)})`);
    await click('Continue');
    const m = await mailWith('/auth/verify', start);
    await go(linkIn(m.body, '/auth/verify'));
    await until(`!!__byText('button', 'Sign in')`, 'the sign-in link');
    await click('Sign in');
    await until(`location.pathname === '/'`, 'home after signing in');
    await until(`document.body.innerText.includes('Updated')`, 'the Updated badge', 20000);
    await sleep(800);
    await shot('92-revise-updated');
    const text = await js(`document.body.innerText`);
    const pay = await js(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim().startsWith('Pay '))`);
    const mine = await js(`fetch('/api/portal/deliveries').then((r) => r.json())`);
    const d = mine.deliveries[0];
    const fee = d.steps.find((s) => s.kind === 'fee');
    note(`Updated shown: ${text.includes('Updated')}; a Pay button: ${pay}; fee step ${fee?.state}; the return: ${d.returnFile?.name}`);
    if (pay || fee?.state !== 'done') throw new Error('the fee is asked for again');
    const viewed = await js(`(async () => { const a = [...document.querySelectorAll('a')].find((e) => e.textContent.trim() === 'View'); if (!a) return 0; const r = await fetch(a.href); return r.status; })()`);
    note(`View of the corrected return answered ${viewed}`);
    if (viewed !== 200) throw new Error(`View answered ${viewed}`);
  } else if (PHASE === 'office-mail') {
    // Owner 2026-11-26: Form 8879 signed at the office (2024) and by mail (2023): the card says
    // what to do; the mailed one has I've mailed it back.
    const start = Date.now() / 1000;
    await go(`${BASE}/login`);
    await until(`!!document.querySelector('input')`, 'the sign-in page');
    await js(`__type(document.querySelector('input'), ${JSON.stringify(EMAIL)})`);
    await click('Continue');
    const m = await mailWith('/auth/verify', start);
    await go(linkIn(m.body, '/auth/verify'));
    await until(`!!__byText('button', 'Sign in')`, 'the sign-in link');
    await click('Sign in');
    await until(`location.pathname === '/'`, 'home after signing in');
    await until(`document.body.innerText.includes('Sign Form 8879 at our office')`, 'the office words', 20000);
    await until(`document.body.innerText.includes('We mailed you your tax return')`, 'the mail words', 20000);
    await sleep(800);
    await shot('95-office-mail-cards');
    const text = await js(`document.body.innerText`);
    note(`Office: ${(text.match(/[^\n]*Sign Form 8879 at our office[^\n]*/) || ['?'])[0]}`);
    note(`Mail: ${(text.match(/[^\n]*We mailed you your tax return[^\n]*/) || ['?'])[0]}`);
    note(`Open eSignature shown: ${await js(`[...document.querySelectorAll('a')].some((a) => a.textContent.trim() === 'Open eSignature')`)}`);
    await click("I've mailed it back");
    await until(`document.body.innerText.includes('You said you mailed it back')`, 'mailed back', 20000);
    await sleep(600);
    await shot('96-mailed-back');
    note('Pressed I\'ve mailed it back: the card says You said you mailed it back');
  } else if (PHASE === 'held') {
    // A balance posted with Hold the client's files: listed, held, Pay now; paid, they open.
    const STRIPE = SECOND;
    const start = Date.now() / 1000;
    await go(`${BASE}/login`);
    await until(`!!document.querySelector('input')`, 'the sign-in page');
    await js(`__type(document.querySelector('input'), ${JSON.stringify(EMAIL)})`);
    await click('Continue');
    const m = await mailWith('/auth/verify', start);
    await go(linkIn(m.body, '/auth/verify'));
    await until(`!!__byText('button', 'Sign in')`, 'the sign-in link');
    await click('Sign in');
    await until(`location.pathname === '/'`, 'home after signing in');
    await until(`document.body.innerText.includes('Balance due')`, 'the balance card');
    await until(`/files open when your balance of/.test(document.body.innerText)`, 'the held note', 20000);
    await sleep(800);
    await shot('80-files-held');
    const text = await js(`document.body.innerText`);
    note(`Held note: ${(text.match(/[^\n]*files open when your balance of[^\n]*/) || ['?'])[0]}`);
    note(`Badges "Opens when your balance is paid": ${(text.match(/Opens when your balance is paid/g) || []).length}`);
    note(`Open or Download buttons shown: ${await js(`[...document.querySelectorAll('a,button')].filter((e) => /^(Open|Download)$/.test(e.textContent.trim())).length`)}`);
    const files = await js(`fetch('/api/portal/files').then((r) => r.json())`);
    note(`Files list: ${files.files.length} files, ${files.files.filter((f) => f.held).length} held; held at the top: ${JSON.stringify(files.held)}`);
    // Owner 2026-11-26: the paid statements stay open; the organizer (on no paid card) is held.
    for (const f of files.files) {
      const r = await js(`fetch('/api/portal/files/${f.id}/content?mode=download').then(async (r) => [r.status, await r.text()])`);
      note(`${f.name}: held ${f.held}; content answers ${r[0]} ${r[0] === 200 ? '' : r[1].slice(0, 80)}`);
      const want = f.name.includes('Organizer') ? 423 : 200;
      if (r[0] !== want || f.held !== (want === 423)) {
        throw new Error(`${f.name}: content ${r[0]}, held ${f.held}; wanted ${want}`);
      }
    }
    // Sharing a held file on (Send a file to someone outside) is refused too.
    const heldOne = files.files.find((f) => f.held);
    if (heldOne) {
      const me = await js(`fetch('/api/portal/auth/me').then((r) => r.json())`);
      const r = await js(`fetch('/api/portal/outside-shares', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ clientId: ${JSON.stringify(me.activeClientId)}, fileIds: [${JSON.stringify(heldOne.id)}], email: 'outside.person@example.com', name: 'Outside Person', days: 7 }) }).then(async (r) => [r.status, await r.text()])`);
      note(`Sending a held file outside answers ${r[0]} ${r[1].slice(0, 80)}`);
      // 403 client_links_off: the firm does not let clients send links (nothing to hold).
      if (r[0] !== 423 && !(r[0] === 403 && r[1].includes('client_links_off'))) {
        throw new Error(`sending a held file outside answered ${r[0]}`);
      }
    }
    // Pay now goes to the balance card on the same page.
    const anchor = await js(`(() => { const a = [...document.querySelectorAll('a')].find((x) => x.textContent.trim() === 'Pay now'); if (!a) return 'no Pay now link'; const t = document.querySelector(a.getAttribute('href')); return t ? 'Pay now goes to: ' + t.textContent.slice(0, 40) : 'Pay now goes nowhere (' + a.getAttribute('href') + ')'; })()`);
    note(anchor);
    await js(`(() => { const b = [...document.querySelectorAll('button')].find((e) => e.textContent.trim().startsWith('Pay by card')); b.click(); return true; })()`);
    let session = null;
    const before = Date.now();
    for (let i = 0; i < 40 && !session; i++) {
      await sleep(250);
      const s = await (await fetch(`${STRIPE}/_last`)).json();
      if (s?.metadata?.balance_id) session = s;
    }
    if (!session) throw new Error('no Checkout Session for the balance');
    const paid = await (
      await fetch(`${STRIPE}/_pay`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId: session.id, balanceId: session.metadata.balance_id, paid: true }),
      })
    ).json();
    note(`Balance paid by card (${Date.now() - before} ms); webhook ${paid.webhook}`);
    await go(`${BASE}/?balance=${session.metadata.balance_id}&session_id=${session.id}`);
    await until(`!/files open when your balance of/.test(document.body.innerText)`, 'the hold lifted', 20000);
    await sleep(800);
    await shot('81-files-released');
    const after = await js(`fetch('/api/portal/files').then((r) => r.json())`);
    note(`After paying: ${after.files.filter((f) => f.held).length} held`);
    for (const f of after.files) {
      const st = await js(`fetch('/api/portal/files/${f.id}/content?mode=download').then((r) => r.status)`);
      note(`${f.name}: content answers ${st}`);
      if (st !== 200) throw new Error(`${f.name} after paying: ${st}`);
    }
  }
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}.json`), JSON.stringify(steps, null, 2));
  await layout.save(join(OUT, `mobile-${PHASE}.json`));
  ws.close();
  edge.kill();
}
