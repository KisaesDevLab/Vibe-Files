#!/usr/bin/env bash
# PA-37 end-to-end check against a real Vibe Portal (KisaesDevLab/Vibe-Portal) on this PC:
# Vibe Files' calls (crates/vf-portal, through examples/portal_e2e) invite two people to a
# client, share a file with one of them, the client downloads it, uploads one back, and the
# upload is fetched into the cabinet's Portal inbox under the client's name and ID.
#
# Needs: docker, node/pnpm, curl, sha256sum; VP=path to the Vibe-Portal checkout
# (default ../Vibe-Portal). The portal's dev database is reset.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
VP="${VP:-$HERE/../Vibe-Portal}"
WORK="$(mktemp -d)"
LOG="$WORK/api.log"
URL="http://localhost:3101"
pass() { printf '  PASS  %s\n' "$*"; }
die() { printf '  FAIL  %s\n' "$*" >&2; exit 1; }

# On Windows the shell's kill does not reach the node process it started: stop whatever
# listens on the portal's port.
free_port() {
    if command -v taskkill >/dev/null 2>&1; then
        netstat -ano | awk '/:3101 .*LISTENING/ {print $NF}' | sort -u | while read -r pid; do
            taskkill //F //T //PID "$pid" >/dev/null 2>&1 || true
        done
    fi
}

cleanup() {
    [ -n "${API_PID:-}" ] && kill "$API_PID" 2>/dev/null || true
    free_port
    docker compose -f "$VP/ops/docker/docker-compose.dev.yml" down >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

echo "== portal: fresh database, API on $URL"
free_port
# A stopped portal can hold the port for a moment.
for _ in $(seq 1 20); do
    netstat -ano 2>/dev/null | grep -q ':3101 .*LISTENING' || break
    sleep 1
done
docker compose -f "$VP/ops/docker/docker-compose.dev.yml" down -v >/dev/null 2>&1 || true
docker compose -f "$VP/ops/docker/docker-compose.dev.yml" up -d --wait >/dev/null 2>&1
(cd "$VP/apps/api" && DATA_DIR="$WORK/data" PORTAL_URL="$URL" npx tsx --env-file-if-exists=../../.env src/server.ts >"$LOG" 2>&1) &
API_PID=$!
for _ in $(seq 1 60); do curl -fs "$URL/health" >/dev/null 2>&1 && break; sleep 1; done
curl -fs "$URL/health" >/dev/null || die "the portal did not start (see $LOG)"
KEY="$(cd "$VP/apps/api" && npx tsx --env-file-if-exists=../../.env src/scripts/firm-key.ts create E2E 2>/dev/null)"     || die "no firm key (see $LOG)"
export VF_PORTAL_URL="$URL" VF_PORTAL_KEY="$KEY"

cargo build -q -p vf-portal --example portal_e2e
VF="$HERE/target/debug/examples/portal_e2e"
"$VF" status >/dev/null || die "Vibe Files cannot reach the portal"
pass "Vibe Files reaches the portal with the firm key"

C=11111111-1111-4111-8111-111111111111
"$VF" client "$C" 1001 "Smith, John"
link() { grep -o "#t=[A-Za-z0-9_-]*" "$LOG" | tail -1 | cut -c4-; }
for who in a b; do
    "$VF" invite "$C" "$who@example.test"
    sleep 1
    curl -fs -c "$WORK/$who.jar" -H 'Content-Type: application/json' \
        "$URL/api/portal/invitations/accept" -d "{\"token\":\"$(link)\"}" >/dev/null
done
pass "two people invited from Vibe Files accepted and signed in"
A_ID="$("$VF" users "$C" | awk '$2=="a@example.test"{print $1}')"
B_ID="$("$VF" users "$C" | awk '$2=="b@example.test"{print $1}')"
[ -n "$A_ID" ] && [ -n "$B_ID" ] || die "users not listed"

ROOT="$WORK/cabinet"
mkdir -p "$ROOT/Smith, John_1001/2025"
DOC="$ROOT/Smith, John_1001/2025/Smith 2025 Form 1040.pdf"
printf '%%PDF-1.7\n%s\n%%%%EOF\n' "a synthetic return" >"$DOC"
FILE="$("$VF" share "$C" doc-1040 "$DOC" "$A_ID" "$B_ID")"
pass "shared to a only"

count() { curl -fs -b "$WORK/$1.jar" "$URL/api/portal/files" | { grep -o '"id":"' || true; } | wc -l | tr -d ' '; }
NA="$(count a)"
NB="$(count b)"
[ "$NA" = 1 ] && [ "$NB" = 0 ] || die "a sees $NA, b sees $NB"
curl -fs -b "$WORK/a.jar" -o "$WORK/got.pdf" "$URL/api/portal/files/$FILE/content"
[ "$(sha256sum <"$WORK/got.pdf" | cut -c1-64)" = "$(sha256sum <"$DOC" | cut -c1-64)" ] || die "download differs"
code="$(curl -s -o /dev/null -w '%{http_code}' -b "$WORK/b.jar" "$URL/api/portal/files/$FILE/content")"
[ "$code" = 404 ] || die "b got $code"
pass "a lists and downloads it unchanged; b does not see it (404)"
"$VF" vis "$FILE" "$B_ID" true
[ "$(count b)" = 1 ] || die "b not shown"
"$VF" vis "$FILE" "$B_ID" false
[ "$(count b)" = 0 ] || die "b still sees it"
code="$(curl -s -o /dev/null -w '%{http_code}' -b "$WORK/b.jar" "$URL/api/portal/files/$FILE/content")"
[ "$code" = 404 ] || die "b got $code after hiding"
pass "shown to b, then hidden from b again (404)"
[ "$("$VF" viewed doc-1040 "$A_ID")" = opened ] || die "not marked opened"
pass "Vibe Files sees that a opened it"

printf '%%PDF-1.7\n%s\n%%%%EOF\n' "a synthetic W-2" >"$WORK/W-2.pdf"
B="$(cat /proc/sys/kernel/random/uuid 2>/dev/null || python -c 'import uuid;print(uuid.uuid4())')"
curl -fs -b "$WORK/a.jar" -H 'Content-Type: application/octet-stream' --data-binary @"$WORK/W-2.pdf" \
    "$URL/api/portal/uploads?clientId=$C&batchId=$B&name=W-2.pdf&note=Second%20job" >/dev/null
mkdir -p "$ROOT/_Vibe"
SAVED="$("$VF" fetch "$ROOT")"
[ "$SAVED" = "_Portal inbox/Smith, John_1001_W-2.pdf" ] || die "saved as '$SAVED'"
[ "$(sha256sum <"$ROOT/$SAVED" | cut -c1-64)" = "$(sha256sum <"$WORK/W-2.pdf" | cut -c1-64)" ] || die "upload differs"
[ -z "$("$VF" fetch "$ROOT")" ] || die "fetched twice"
grep -q '"note":"Second job"' "$ROOT/_Vibe/portal/received.jsonl" || die "note not logged"
pass "a's upload is fetched once into _Portal inbox as 'Smith, John_1001_W-2.pdf', with its note"
curl -fs -b "$WORK/a.jar" "$URL/api/portal/uploads" | grep -q '"received":true' || die "client not told"
pass "the client sees it as received by the firm"

# Two PCs fetching at the same moment: every upload arrives exactly once, nothing left behind.
for n in 1 2 3; do
    printf '%%PDF-1.7\n%s\n%%%%EOF\n' "synthetic 1099 $n" >"$WORK/1099-$n.pdf"
    curl -fs -b "$WORK/a.jar" -H 'Content-Type: application/octet-stream' \
        --data-binary @"$WORK/1099-$n.pdf" \
        "$URL/api/portal/uploads?clientId=$C&batchId=$B&name=1099.pdf" >/dev/null
done
(VF_PC=PC-ONE "$VF" fetch "$ROOT" >"$WORK/one.txt" 2>&1 || true) &
P1=$!
(VF_PC=PC-TWO "$VF" fetch "$ROOT" >"$WORK/two.txt" 2>&1 || true) &
P2=$!
# Only these two: a bare `wait` would also wait for the portal server.
wait "$P1" "$P2"
VF_PC=PC-ONE "$VF" fetch "$ROOT" >>"$WORK/one.txt" 2>&1 || true
N="$(ls "$ROOT/_Portal inbox" | grep -c '1099')"
[ "$N" = 3 ] || die "$N copies of the three 1099s arrived ($(ls "$ROOT/_Portal inbox" | tr '\n' ','))"
for n in 1 2 3; do
    got="$(for f in "$ROOT/_Portal inbox/"*1099*; do sha256sum <"$f" | cut -c1-64; done | sort)"
    echo "$got" | grep -q "$(sha256sum <"$WORK/1099-$n.pdf" | cut -c1-64)" || die "1099 $n missing or damaged"
done
[ -z "$(ls "$ROOT/_Vibe/tmp/portal" 2>/dev/null)" ] || die "left behind: $(ls "$ROOT/_Vibe/tmp/portal")"
pass "two PCs fetching at once: three uploads of the same name arrive once each, intact, named apart"
echo "== all passed"
