// Organizer links walkthrough (PA-113), client side: drives the real portal pages in headless
// Edge (DevTools protocol). Accepts each invitation to EMAIL in the portal's log (EMAIL_PROVIDER
// =console), as a client reads their email, then checks the home page's Organizer card for each
// client the person has. Synthetic data only.
//   node client.mjs <base> <outDir> <apiLog> <email> <phase> <expect.json>
// expect.json: [{ client: "<name>", card: "none" | "in_progress" | "submitted" | ..., link? }]
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { W, H, emulate, layoutLog } from '../walkthrough/mobile.mjs';

const [BASE, OUT, LOG, EMAIL, PHASE, EXPECT] = process.argv.slice(2);
const expect = JSON.parse(await readFile(EXPECT, 'utf8'));
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const DPORT = 9351;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const steps = [];
const note = (s) => {
  steps.push(s);
  console.log(s);
};

const profile = await mkdtemp(join(tmpdir(), 'edge-org-'));
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
const errors = [];
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.method === 'Runtime.exceptionThrown') errors.push(JSON.stringify(m.params.exceptionDetails).slice(0, 300));
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
await send('Runtime.enable');
await emulate(send);
const layout = layoutLog(js, writeFile);
await send('Page.addScriptToEvaluateOnNewDocument', {
  source: `
    window.__byText = (sel, text, root = document) =>
      [...root.querySelectorAll(sel)].find((e) => e.textContent.trim() === text && !e.disabled);
    window.__card = () => [...document.querySelectorAll('section, [id^=organizer-]')]
      .find((e) => /tax organizer/.test(e.querySelector('h2, h3')?.textContent ?? ''));
  `,
});
const go = async (url) => {
  const r = await send('Page.navigate', { url });
  if (r.error) throw new Error(`navigate ${url}: ${r.error.message}`);
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
const TAG = process.env.VF_MOBILE === '1' ? 'phone' : 'desktop';
const shot = async (name) => {
  await layout.check(name);
  const { result } = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
  await writeFile(join(OUT, `${name}-${TAG}.png`), Buffer.from(result.data, 'base64'));
};
const invites = async () => {
  const lines = (await readFile(LOG, 'utf8')).split('\n').filter((l) => l.includes('"msg":"notify ('));
  const links = lines
    .map((l) => JSON.parse(l))
    .filter((m) => m.to === EMAIL && m.body?.includes('/invite'))
    .map((m) => m.body.match(/https?:\/\/\S*\/invite\S*/)[0]);
  return [...new Set(links)];
};

try {
  let links = [];
  for (let i = 0; i < 40 && links.length < expect.length; i++) {
    links = await invites();
    if (links.length < expect.length) await sleep(250);
  }
  note(`${links.length} invitation(s) to ${EMAIL}`);
  // Accepting one invitation gives the person every invitation to the same email: the rest
  // then say "Invitation not valid", as they would in their mail.
  let accepted = 0;
  for (const link of links) {
    await go(link);
    await until(
      `!!__byText('button', 'Accept and sign in') || !!__byText('button', 'Accept') || document.body.innerText.includes('Invitation not valid')`,
      'invitation page',
    );
    if (await js(`document.body.innerText.includes('Invitation not valid')`)) continue;
    await js(`(__byText('button', 'Accept and sign in') ?? __byText('button', 'Accept')).click()`);
    await until(`location.pathname === '/'`, 'home after accepting');
    accepted += 1;
  }
  if (!accepted) throw new Error('no invitation could be accepted');
  await go(`${BASE}/`);
  await until(`!!document.querySelector('h1')`, 'home');
  note('Invitations accepted: signed in');
  for (const [i, e] of expect.entries()) {
    // Choose the client (the chooser is there when the person has more than one).
    const many = await js(`!!document.querySelector('select')`);
    if (many) {
      const ok = await js(`(() => { const s = [...document.querySelectorAll('select')].find((x) => [...x.options].some((o) => o.textContent.trim() === ${JSON.stringify(e.client)}));
        if (!s) return false; const o = [...s.options].find((o) => o.textContent.trim() === ${JSON.stringify(e.client)});
        const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, o.value);
        s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
      if (!ok) throw new Error(`no client ${e.client} in the chooser`);
    }
    await until(`document.querySelector('h1')?.textContent.trim() === ${JSON.stringify(e.client)}`, `showing ${e.client}`);
    // Give the cards time to read.
    await sleep(2500);
    const card = await js(`(() => { const c = __card(); if (!c) return null; const a = c.querySelector('a');
      return { title: c.querySelector('h2, h3')?.textContent.trim(), text: c.innerText, href: a?.href ?? null, target: a?.target ?? null, rel: a?.rel ?? null }; })()`);
    if (e.card === 'none') {
      if (card) throw new Error(`${e.client}: a card is shown and should not be: ${card.text}`);
      note(`${e.client}: no Organizer card (as expected)`);
    } else {
      if (!card) throw new Error(`${e.client}: no Organizer card`);
      const words = { in_progress: 'In progress', not_seen: 'Not started', submitted: 'Submitted' }[e.card];
      if (words && !card.text.includes(words)) throw new Error(`${e.client}: the card does not say ${words}: ${card.text}`);
      if (e.card === 'submitted' && !card.text.includes('Thank you')) throw new Error(`${e.client}: no thank-you`);
      if (e.link && card.href !== e.link) throw new Error(`${e.client}: link ${card.href}, not ${e.link}`);
      if (card.target !== '_blank' || !/noopener/.test(card.rel)) throw new Error(`${e.client}: the link does not open safely in a new tab`);
      note(`${e.client}: "${card.title}" · ${words} · link opens ${new URL(card.href).host} in a new tab`);
    }
    await shot(`${PHASE}-${i + 1}`);
  }
  if (errors.length) throw new Error(`page errors: ${errors.join(' | ')}`);
} catch (e) {
  note(`ERROR: ${e.message}`);
  process.exitCode = 1;
} finally {
  await writeFile(join(OUT, `client-${PHASE}-${TAG}.json`), JSON.stringify(steps, null, 2));
  await layout.save(join(OUT, `mobile-${PHASE}-${TAG}.json`));
  ws.close();
  edge.kill();
}
