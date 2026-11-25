// Load test (Addendum 02, owner 2026-11-24): N synthetic clients, each with a delivery carrying
// letters' signers, a fee and Form 8879, made through the firm API of a test portal; then the
// deliveries list is timed. Synthetic names only.
//   node load.mjs <portal url> <firm key> [N]
const [url, key, nArg] = process.argv.slice(2);
const N = Number(nArg ?? 500);
const api = `${url}/api/firm/v1`;
const h = { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' };
const call = async (method, path, body) => {
  const r = await fetch(api + path, { method, headers: h, body: body && JSON.stringify(body) });
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${await r.text()}`);
  return r.json();
};
const uuid = () => crypto.randomUUID();
const t0 = Date.now();
const ids = [];
for (let i = 0; i < N; i += 1) {
  const id = uuid();
  ids.push(id);
  await call('PUT', `/clients/${id}`, { clientCode: `L${1000 + i}`, displayName: `Loadtest Client ${i}` });
  const { delivery } = await call('POST', '/deliveries', {
    clientId: id,
    taxYear: 2025,
    returnType: i % 4 === 0 ? '1065' : '1040',
    steps: { fee: true, esign8879: true },
    remindDays: 3,
  });
  await call('PUT', `/deliveries/${delivery.id}/fee`, {
    mode: 'amount',
    lines: [{ key: 'fee', kind: 'base', name: 'Tax return preparation', cents: 50000 + i }],
  });
  await call('PUT', `/deliveries/${delivery.id}/esign-link`, { url: 'https://esign.example-tax.test/x' });
}
const made = Date.now() - t0;
const times = [];
let bytes = 0;
for (let k = 0; k < 5; k += 1) {
  const s = Date.now();
  const r = await fetch(`${api}/deliveries`, { headers: h });
  const text = await r.text();
  bytes = text.length;
  times.push(Date.now() - s);
}
const s2 = Date.now();
await fetch(`${api}/deliveries?status=open&taxYear=2025&returnType=1065`, { headers: h }).then((r) => r.text());
const filtered = Date.now() - s2;
console.log(JSON.stringify({ n: N, madeMs: made, listMs: times, listBytes: bytes, filteredMs: filtered }));
