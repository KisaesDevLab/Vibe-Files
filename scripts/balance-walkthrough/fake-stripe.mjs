// A stand-in for Stripe's API for the walkthrough (synthetic only): Checkout Sessions created
// form-encoded and retrieved, and POST /_pay { sessionId, balanceId, paid } that completes a
// session and sends the portal a webhook signed as Stripe signs them (HMAC-SHA256 of
// "t.body" with the whsec secret).
// Usage: node fake-stripe.mjs <port> <secret key> <whsec> <portal webhook url>
import { createServer } from 'node:http';
import { createHmac } from 'node:crypto';

const [port, secret, whsec, hookUrl] = process.argv.slice(2);
const sessions = new Map();
let n = 0;

function sign(raw) {
  const t = Math.floor(Date.now() / 1000);
  const v1 = createHmac('sha256', whsec).update(`${t}.${raw}`).digest('hex');
  return `t=${t},v1=${v1}`;
}

createServer((req, res) => {
  let raw = '';
  req.on('data', (c) => (raw += c));
  req.on('end', async () => {
    res.setHeader('Content-Type', 'application/json');
    const url = new URL(req.url, 'http://x');
    if (url.pathname === '/_last') {
      res.end(JSON.stringify([...sessions.values()].at(-1) ?? null));
      return;
    }
    if (url.pathname === '/_pay') {
      const { sessionId, balanceId, paid } = JSON.parse(raw || '{}');
      const s = {
        id: sessionId,
        status: 'complete',
        payment_status: paid ? 'paid' : 'unpaid',
        metadata: { balance_id: balanceId, method: 'card' },
      };
      sessions.set(sessionId, s);
      const body = JSON.stringify({
        id: `evt_walk_${Date.now()}`,
        type: 'checkout.session.completed',
        data: { object: s },
      });
      const r = await fetch(hookUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Stripe-Signature': sign(body) },
        body,
      });
      res.end(JSON.stringify({ webhook: r.status }));
      return;
    }
    const auth = Buffer.from((req.headers.authorization ?? '').replace(/^Basic /, ''), 'base64')
      .toString()
      .replace(/:$/, '');
    if (auth !== secret) {
      res.statusCode = 401;
      res.end(JSON.stringify({ error: { message: 'Invalid API Key provided' } }));
      return;
    }
    if (req.method === 'POST' && url.pathname === '/v1/checkout/sessions') {
      const f = new URLSearchParams(raw);
      n += 1;
      const id = `cs_test_walk_${n}`;
      const metadata = {};
      for (const [k, v] of f) {
        const m = /^metadata\[(.+)\]$/.exec(k);
        if (m) metadata[m[1]] = v;
      }
      const s = { id, url: `https://checkout.stripe.test/c/pay/${id}`, status: 'open', payment_status: 'unpaid', metadata };
      sessions.set(id, s);
      res.end(JSON.stringify(s));
      return;
    }
    const m = /^\/v1\/checkout\/sessions\/(.+)$/.exec(url.pathname);
    if (req.method === 'GET' && m) {
      const s = sessions.get(decodeURIComponent(m[1]));
      res.statusCode = s ? 200 : 404;
      res.end(JSON.stringify(s ?? { error: { message: 'No such checkout.session' } }));
      return;
    }
    if (req.method === 'GET' && url.pathname === '/v1/checkout/sessions') {
      res.end(JSON.stringify({ object: 'list', data: [] }));
      return;
    }
    res.statusCode = 404;
    res.end('{}');
  });
}).listen(Number(port), '127.0.0.1');
