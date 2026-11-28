// Walkthrough, client side: drives the real portal pages in headless Edge (DevTools protocol).
//   node client.mjs <base> <outDir> <apiLog> <email> <phase: answer|after> <pdf>
// Sign-in links are read from the portal's log (EMAIL_PROVIDER=console), as a client reads
// their email. Writes PNGs and a JSON step log to outDir.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { W, H, emulate } from '../walkthrough/mobile.mjs';

const [BASE, OUT, LOG, EMAIL, PHASE, PDF] = process.argv.slice(2);
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const DPORT = 9344;
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
// Training 2026-11-27: VF_TRAIN=1 adds phone-screen captures (train-*.png, the visible screen
// only, light), named for the video's scenes.
const TRAIN = process.env.VF_TRAIN === '1';
if (TRAIN) await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });

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
    window.__item = (part) => [...document.querySelectorAll('li.p-todo-item')].find((li) => (li.getAttribute('aria-label') || '').includes(part));
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
const tshot = async (name, text = null) => {
  if (!TRAIN) return;
  await js(text
    ? `(() => { const e = [...document.querySelectorAll('body *')].reverse().find((x) => x.children.length === 0 && x.textContent.includes(${JSON.stringify(text)})); if (e) e.scrollIntoView({ block: 'center' }); return true; })()`
    : 'window.scrollTo(0, 0)');
  await sleep(500);
  const v = await js('({ x: scrollX, y: scrollY, w: innerWidth, h: innerHeight })');
  const { result } = await send('Page.captureScreenshot', { format: 'png', clip: { x: v.x, y: v.y, width: v.w, height: v.h, scale: 1 } });
  await writeFile(join(OUT, `train-${name}.png`), Buffer.from(result.data, 'base64'));
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

async function signIn() {
  const t0 = Date.now();
  await go(`${BASE}/login`);
  const input = `[...document.querySelectorAll('input')].find((i) => i.type !== 'checkbox')`;
  await until(`!!(${input})`, 'login page');
  await js(`__type(${input}, ${JSON.stringify(EMAIL)})`);
  await click('Continue');
  await sleep(800);
  await shot(`${PHASE === 'answer' ? '05' : '12'}-client-login-link-sent`);
  const mail = await mailWith('/auth/verify#t=', t0);
  note(`Sign-in email to ${EMAIL}: "${mail.subject}"`);
  await go(linkIn(mail.body, '/auth/verify'));
  await until(`!!__byText('button', 'Sign in')`, 'verify page');
  await click('Sign in');
  await until(`location.pathname === '/' && document.body.innerText.length > 50`, 'home after sign-in');
  await sleep(1200);
}

try {
  if (PHASE === 'accept') {
    // Before the walkthrough: the client accepts the firm's portal invitation (made from
    // Vibe Files), as an existing portal client already has.
    const inv = await mailWith('/invite', 0);
    note(`Invitation email to ${EMAIL}: "${inv.subject}"`);
    await go(linkIn(inv.body, '/invite'));
    await until(`!!__byText('button', 'Accept and sign in')`, 'invitation page');
    await shot('00-client-invitation');
    await click('Accept and sign in');
    await until(`location.pathname === '/'`, 'home after accepting');
    note('Invitation accepted: the client has a portal account');
  } else if (PHASE === 'answer') {
    // The to-do emails the client received (steps 2 and 4).
    for (const [what, has] of [['To-do list sent', 'has sent you a to-do list'], ['Reminder', 'still open on']]) {
      const m = await mailWith(has, 0);
      note(`${what} email to ${EMAIL}: subject "${m.subject}" / text "${m.body}"`);
    }
    // Step 5: signing in from the login page, by the emailed link.
    await signIn();
    await shot('06-client-home-to-do');
    await tshot('t01-home', 'To do');
    note(`Home page: ${(await js(`document.body.innerText`)).split('\n').filter((l) => /to do|of \d+ done/i.test(l)).join(' | ')}`);

    // Open the list.
    await click('Open');
    await until(`document.querySelectorAll('li.p-todo-item').length > 0`, 'list items');
    await shot('07-client-list-open');
    await tshot('t02-list');
    const items = await js(`[...document.querySelectorAll('li.p-todo-item')].map((li) => li.getAttribute('aria-label'))`);
    note(`Items shown to the client: ${items.length}`);

    // Step 6: upload a document for the first W-2 item, through the page's file input.
    const w2 = items.find((l) => l.startsWith('W-2'));
    const { result: inputNode } = await send('Runtime.evaluate', {
      expression: `__item(${JSON.stringify(w2)}).querySelector('input[type=file]')`,
    });
    await send('DOM.setFileInputFiles', { files: [PDF], objectId: inputNode.result.objectId });
    await until(`!!__byText('button', 'Send another file', __item(${JSON.stringify(w2)}))`, 'upload done', 30000);
    await shot('08-client-uploaded');
    await tshot('t03-uploaded', 'Send another file');
    note(`Uploaded ${PDF.split(/[\\/]/).pop()} for "${w2}"`);

    // Step 7: answer a question, and mark a document as not applying.
    const q = items.find((l) => l.startsWith('Any changes'));
    await js(`__byText('button', 'Answer', __item(${JSON.stringify(q)})).click()`);
    await sleep(500);
    await js(`__type(__item(${JSON.stringify(q)}).querySelector('textarea'), 'We moved in June: new address is 12 Sample Lane, Exampleville.')`);
    await js(`__byText('button', 'Send answer', __item(${JSON.stringify(q)})).click()`);
    await until(`!!__byText('button', 'Change answer', __item(${JSON.stringify(q)}))`, 'answer sent');
    note(`Answered "${q}"`);
    await tshot('t04-answered', 'Change answer');
    const na = items.find((l) => l.startsWith('Form 1098'));
    await js(`__byText('button', 'Does not apply', __item(${JSON.stringify(na)})).click()`);
    await sleep(500);
    await js(`__type(__item(${JSON.stringify(na)}).querySelector('textarea'), 'We paid off the mortgage in 2025.')`);
    await js(`__byText('button', 'Send', __item(${JSON.stringify(na)})).click()`);
    await until(`!__byText('button', 'Does not apply', __item(${JSON.stringify(na)}))`, 'not applicable sent');
    note(`Marked "${na}" as not applying`);
    await sleep(800);
    await shot('09-client-answered');
    await tshot('t05-not-apply', 'Form 1098');
    note(`Progress: ${await js(`[...document.querySelectorAll('[aria-label*=" done"]')].map((e) => e.getAttribute('aria-label')).join(' | ')`)}`);
  } else {
    // After the firm cancelled one list and deleted the other.
    await signIn();
    await shot('15-client-after-cancel');
    const text = await js(`document.body.innerText`);
    note(`Client home after cancel/delete: to-do lists shown: ${(text.match(/of \d+ done/g) || []).length}`);
  }
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}.json`), JSON.stringify(steps, null, 2));
  ws.close();
  edge.kill();
}
