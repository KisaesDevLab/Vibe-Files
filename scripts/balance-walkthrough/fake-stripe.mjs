// A stand-in for Stripe's API for the walkthrough (synthetic only): Checkout Sessions created
// form-encoded and retrieved, and POST /_pay { sessionId, balanceId, paid } that completes a
// session and sends the portal a webhook signed as Stripe signs them (HMAC-SHA256 of
// "t.body" with the whsec secret).
// Annual agreements (2026-11-25): a session in subscription mode makes a subscription when paid
// (GET/POST/DELETE /v1/subscriptions/:id), its invoices listed (GET /v1/invoices), and
// POST /_month { subscription, paid } sends invoice.paid or invoice.payment_failed for a month.
// Usage: node fake-stripe.mjs <port> <secret key> <whsec> <portal webhook url>
import { createServer } from 'node:http';
import { createHmac } from 'node:crypto';

const [port, secret, whsec, hookUrl] = process.argv.slice(2);
const sessions = new Map();
const subs = new Map();
const invoices = [];
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
    const hook = async (type, object) => {
      const body = JSON.stringify({ id: `evt_walk_${Date.now()}_${Math.random()}`, type, data: { object } });
      const r = await fetch(hookUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Stripe-Signature': sign(body) },
        body,
      });
      return r.status;
    };
    if (url.pathname === '/_settle') {
      // A bank payment clears: checkout.session.async_payment_succeeded.
      const { sessionId } = JSON.parse(raw || '{}');
      const s = { ...sessions.get(sessionId), payment_status: 'paid' };
      sessions.set(sessionId, s);
      res.end(JSON.stringify({ webhook: await hook('checkout.session.async_payment_succeeded', s) }));
      return;
    }
    if (url.pathname === '/_month') {
      const { subscription, paid } = JSON.parse(raw || '{}');
      const sub = subs.get(subscription);
      const cents = sub?.items.data[0].price.unit_amount ?? 0;
      const inv = {
        id: `in_walk_${invoices.length + 1}`,
        status: paid ? 'paid' : 'open',
        amount_paid: paid ? cents : 0,
        amount_due: cents,
        billing_reason: 'subscription_cycle',
        parent: { type: 'subscription_details', subscription_details: { subscription } },
        status_transitions: { paid_at: paid ? Math.floor(Date.now() / 1000) : null },
      };
      invoices.unshift(inv);
      res.end(JSON.stringify({ webhook: await hook(paid ? 'invoice.paid' : 'invoice.payment_failed', inv) }));
      return;
    }
    if (url.pathname === '/_pay') {
      const { sessionId, balanceId, paid } = JSON.parse(raw || '{}');
      const made = sessions.get(sessionId);
      const s = {
        ...(made ?? {}),
        id: sessionId,
        status: 'complete',
        payment_status: paid ? 'paid' : 'unpaid',
        // The session's own metadata when the portal made it (a delivery's), else a balance's.
        metadata: made?.metadata ?? { balance_id: balanceId, method: 'card' },
      };
      if (made?.mode === 'subscription') {
        const id = `sub_walk_${subs.size + 1}`;
        subs.set(id, {
          id,
          status: 'active',
          customer: 'cus_walk_1',
          metadata: made.subscription_metadata,
          items: { data: [{ id: 'si_walk_1', price: { unit_amount: made.monthly, product: 'prod_walk_1' }, current_period_end: Math.floor(Date.now() / 1000) + 30 * 86400 }] },
        });
        s.subscription = id;
        s.customer = 'cus_walk_1';
      }
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
      const subscription_metadata = {};
      for (const [k, v] of f) {
        const m = /^metadata\[(.+)\]$/.exec(k);
        if (m) metadata[m[1]] = v;
        const sm = /^subscription_data\[metadata\]\[(.+)\]$/.exec(k);
        if (sm) subscription_metadata[sm[1]] = v;
      }
      const mode = f.get('mode') ?? 'payment';
      // The monthly line: the one with recurring[interval].
      let monthly = 0;
      for (const [k] of f) {
        const r = /^line_items\[(\d+)\]\[price_data\]\[recurring\]\[interval\]$/.exec(k);
        if (r) monthly = Number(f.get(`line_items[${r[1]}][price_data][unit_amount]`));
      }
      const s = { id, url: `https://checkout.stripe.test/c/pay/${id}`, status: 'open', payment_status: 'unpaid', mode, metadata, subscription_metadata, monthly };
      sessions.set(id, s);
      res.end(JSON.stringify(s));
      return;
    }
    // Expire (End engagement, Cancel, a new price): an open session can no longer be paid; a
    // completed one is refused, as Stripe does.
    const ex = /^\/v1\/checkout\/sessions\/(.+)\/expire$/.exec(url.pathname);
    if (req.method === 'POST' && ex) {
      const s = sessions.get(decodeURIComponent(ex[1]));
      if (s && s.status === 'open') {
        s.status = 'expired';
        res.end(JSON.stringify(s));
      } else {
        res.statusCode = 400;
        res.end(JSON.stringify({ error: { message: 'Only open sessions can be expired' } }));
      }
      return;
    }
    const m = /^\/v1\/checkout\/sessions\/(.+)$/.exec(url.pathname);
    if (req.method === 'GET' && m) {
      const s = sessions.get(decodeURIComponent(m[1]));
      res.statusCode = s ? 200 : 404;
      res.end(JSON.stringify(s ?? { error: { message: 'No such checkout.session' } }));
      return;
    }
    const sm = /^\/v1\/subscriptions\/(.+)$/.exec(url.pathname);
    if (sm) {
      const s = subs.get(decodeURIComponent(sm[1]));
      if (!s) {
        res.statusCode = 404;
        res.end(JSON.stringify({ error: { message: 'No such subscription' } }));
        return;
      }
      if (req.method === 'DELETE') s.status = 'canceled';
      if (req.method === 'POST') {
        const f = new URLSearchParams(raw);
        const a = f.get('items[0][price_data][unit_amount]');
        if (a) s.items.data[0].price = { unit_amount: Number(a), product: f.get('items[0][price_data][product]') };
        if (f.get('default_payment_method')) s.default_payment_method = f.get('default_payment_method');
      }
      res.end(JSON.stringify(s));
      return;
    }
    if (req.method === 'GET' && url.pathname === '/v1/invoices') {
      const sub = url.searchParams.get('subscription');
      res.end(JSON.stringify({ object: 'list', data: invoices.filter((i) => i.parent.subscription_details.subscription === sub) }));
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
