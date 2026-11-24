#!/bin/sh
# Balance owed walkthrough (Addendum §16): a fresh test portal on :3399 (Postgres container
# vf-walk4-pg on :5547) with a stand-in for Stripe on :3398. Synthetic data and fake keys only.
#   sh reset-portal.sh <Vibe-Portal checkout> <work dir>
# Writes <work dir>/walk4-key.txt (the firm key) and <work dir>/walk4-api.log (the portal's log).
# Send its output to a file, not a pipe: the portal and the stand-in keep running in the background.
# Then: VF_WALK_URL=http://127.0.0.1:3399 VF_WALK_KEY=$(cat <work>/walk4-key.txt)
#   VF_WALK_STRIPE=http://127.0.0.1:3398 VF_WALK_LOG=<work>/walk4-api.log
#   VF_WALK_CLIENT=<this folder>/client.mjs VF_WALK_DIR=<out>
#   cargo test -p vf-app balance_walkthrough -- --ignored --nocapture
HERE=$(cd "$(dirname "$0")" && pwd)
W=${1:?the Vibe-Portal checkout}
S=${2:?a work folder}
mkdir -p "$S"
SK=sk_test_walkthroughwalkthrough
WH=whsec_walkthroughwalkthrough
for p in 3399 3398; do
  for pid in $(netstat -ano | grep ":$p " | grep LISTENING | awk '{print $5}' | sort -u); do taskkill //PID $pid //T //F >/dev/null 2>&1; done
done
docker rm -f vf-walk4-pg >/dev/null 2>&1
docker run -d --name vf-walk4-pg -e POSTGRES_USER=vibe -e POSTGRES_PASSWORD=vibe -e POSTGRES_DB=walk -p 5547:5432 postgres:16-alpine >/dev/null
for i in $(seq 1 30); do docker exec vf-walk4-pg pg_isready -U vibe -d walk >/dev/null 2>&1 && break; sleep 1; done; sleep 2
cd "$W" || exit 1
export DATABASE_URL=postgres://vibe:vibe@127.0.0.1:5547/walk
pnpm --filter @vibe-portal/db migrate >/dev/null 2>&1 || { echo migrate failed; exit 1; }
pnpm --filter @vibe-portal/api firm-key create "Walkthrough PC" 2>/dev/null | tail -1 > "$S/walk4-key.txt"
docker exec vf-walk4-pg psql -U vibe -d walk -q -c "update firm set name='Sample CPA Firm'" -c "insert into staff_users(email,name,admin,all_clients) values ('pat.preparer@example.com','Pat Preparer',true,true)"
D="$S/walk4-data-$(date +%s)"; mkdir -p "$D"; : > "$S/walk4-api.log"
# The Stripe test keys are sealed with the portal's own key (as Settings › Payments does).
cp "$HERE/walk-seal.ts" apps/api/walk-seal.ts
(cd apps/api && npx tsx walk-seal.ts "$D" $SK $WH); rm -f apps/api/walk-seal.ts
nohup node "$HERE/fake-stripe.mjs" 3398 $SK $WH http://127.0.0.1:3399/api/stripe/webhook > "$S/walk4-stripe.log" 2>&1 &
cd apps/api && PORT=3399 PORTAL_URL=http://127.0.0.1:3399 DATA_DIR="$D" STATIC_DIR="$W/apps/portal/dist" STRIPE_API_BASE=http://127.0.0.1:3398 NODE_ENV=development nohup npx tsx src/server.ts > "$S/walk4-api.log" 2>&1 &
for i in $(seq 1 40); do curl -s -o /dev/null http://127.0.0.1:3399/login && break; sleep 1; done
curl -s -H "Authorization: Bearer $(cat "$S/walk4-key.txt")" http://127.0.0.1:3399/api/firm/v1/status; echo
