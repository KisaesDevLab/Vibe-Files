// Balance owed walkthrough only: copied into the portal's apps/api by reset-portal.sh, run, and
// removed. Seals synthetic Stripe test keys into the firm row and turns
// balance cards on. Usage: tsx walk-seal.ts <data dir> <secret key> <whsec>
import { createDb, schema } from '@vibe-portal/db';

import { openKeyring } from './src/files/keyring';

const [dataDir, secret, whsec] = process.argv.slice(2);
const { db } = createDb({ connectionString: process.env['DATABASE_URL'] as string });
const k = await openKeyring(db, dataDir as string);
const wrap = (v: string): Buffer => Buffer.from(k.wrap(Buffer.from(v, 'utf8')));
await db.update(schema.firm).set({
  stripeSecretEnc: wrap(secret as string),
  stripeWebhookEnc: wrap(whsec as string),
  balancesEnabled: true,
  balanceDaysOn: true,
});
console.log('sealed');
process.exit(0);
