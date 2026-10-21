#!/usr/bin/env bash
#
# End-to-end smoke test for the Ask Aunova MCP server.
#
# Builds the server, starts it on loopback, and checks the things that have to
# be true for the server to be what it claims:
#
#   1. `initialize` answers with protocol version 2025-11-25
#   2. `tools/list` declares all six tools
#   3. a tool answers from the signed record and cites a document for the fact
#   4. a buyer token is refused another unit's payments, and a developer is not
#   5. the evidence card is served as a ui:// resource
#   6. `verify_claim` proves one named field against the record, says so when
#      it differs, and refuses an argument it cannot read — a value outside a
#      closed enum comes back as a JSON-RPC error with no verdict in it at all
#   7. `ask_project` states one topic for the caller's own unit and refuses
#      another unit rather than substituting theirs
#   8. `whats_changed` hands back a cursor, honours it, and remembers nothing
#      without it
#   9. a request with no Authorization header is refused with HTTP 401
#
# Any check that does not produce what it expects fails the script, so this is a
# gate and not just a printer. The server is always stopped by its own PID.
#
# Usage:  bash scripts/smoke.sh
# Env:    ASK_AUNOVA_PORT (default 8731), ASK_AUNOVA_BUYER_TOKEN,
#         ASK_AUNOVA_DEVELOPER_TOKEN, CARGO_TARGET_DIR (default ./target)

set -eu

# Run from the repository root, whatever directory the script was called from.
cd "$(dirname "$0")/.."

PORT="${ASK_AUNOVA_PORT:-8731}"
BASE_URL="http://127.0.0.1:${PORT}/mcp"
TOKEN="${ASK_AUNOVA_BUYER_TOKEN:-demo-token-buyer-1204}"
DEVELOPER_TOKEN="${ASK_AUNOVA_DEVELOPER_TOKEN:-demo-token-developer}"
PROTOCOL_VERSION="2025-11-25"
WORK_DIR=".qa/smoke"

mkdir -p "$WORK_DIR" .qa/tmp

# Keep every build artefact and scratch file inside the repository, without
# overriding a target directory the caller chose.
export TMPDIR="$PWD/.qa/tmp"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
SERVER_BIN="${CARGO_TARGET_DIR}/debug/ask-aunova-server"

SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

fail() {
    printf '\nSMOKE FAILED: %s\n' "$1" >&2
    if [ -s "$WORK_DIR/server.log" ]; then
        printf '\n--- server log ---\n' >&2
        cat "$WORK_DIR/server.log" >&2
    fi
    exit 1
}

step() {
    printf '\n=== %s ===\n' "$1"
}

# post <token> <output-file> <json-body> [extra curl args...]
post() {
    local token="$1" output="$2" body="$3"
    shift 3
    curl -sS -m 10 \
        -o "$output" \
        -H "Authorization: Bearer ${token}" \
        -H 'Accept: application/json, text/event-stream' \
        -H 'Content-Type: application/json' \
        "$@" \
        --data-binary "$body" \
        "$BASE_URL"
}

step "1. build"
cargo build -p ask-aunova-server

step "2. start the server on 127.0.0.1:${PORT}"
[ -x "$SERVER_BIN" ] || fail "$SERVER_BIN was not built"
ASK_AUNOVA_PORT="$PORT" "$SERVER_BIN" >"$WORK_DIR/server.log" 2>&1 &
SERVER_PID=$!
printf 'server pid %s\n' "$SERVER_PID"

step "3. wait for the port to accept connections"
# The probe is deliberately sent without a token: it comes straight back as 401
# without opening an MCP session or a long-lived stream, so it answers "is the
# socket accepting?" and nothing else. The refusal it logs is the server working.
READY=""
for _attempt in $(seq 1 50); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        fail "the server exited before it accepted a connection"
    fi
    if curl -sS -o /dev/null -m 2 "$BASE_URL" 2>/dev/null; then
        READY="yes"
        break
    fi
    sleep 0.2
done
[ -n "$READY" ] || fail "127.0.0.1:${PORT} did not accept a connection within 10 seconds"
cat "$WORK_DIR/server.log"

if command -v ss >/dev/null 2>&1; then
    printf '\nlistening sockets on port %s:\n' "$PORT"
    ss -ltn | grep ":${PORT}\b" || true
fi

step "4. initialize"
post "$TOKEN" "$WORK_DIR/initialize.body" \
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"${PROTOCOL_VERSION}\",\"capabilities\":{},\"clientInfo\":{\"name\":\"smoke.sh\",\"version\":\"0.1.0\"}}}" \
    -D "$WORK_DIR/initialize.headers" || fail "the initialize request did not complete"
cat "$WORK_DIR/initialize.body"

grep -q "\"protocolVersion\":\"${PROTOCOL_VERSION}\"" "$WORK_DIR/initialize.body" ||
    fail "initialize did not answer with protocolVersion ${PROTOCOL_VERSION}"
printf '\nOK: initialize answered with protocolVersion %s\n' "$PROTOCOL_VERSION"

# Carry the session the server handed out, if it handed one out.
SESSION_ID="$(grep -i '^mcp-session-id:' "$WORK_DIR/initialize.headers" |
    cut -d' ' -f2- | tr -d '\r\n')"
SESSION_HEADERS=()
if [ -n "$SESSION_ID" ]; then
    printf 'OK: session %s\n' "$SESSION_ID"
    SESSION_HEADERS=(-H "Mcp-Session-Id: ${SESSION_ID}"
        -H "MCP-Protocol-Version: ${PROTOCOL_VERSION}")
fi

step "5. tools/list"
post "$TOKEN" "$WORK_DIR/tools-list.body" \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    "${SESSION_HEADERS[@]}" || fail "the tools/list request did not complete"
cat "$WORK_DIR/tools-list.body"

for tool in get_milestones get_certificates get_payment_status \
    verify_claim ask_project whats_changed; do
    grep -q "\"${tool}\"" "$WORK_DIR/tools-list.body" ||
        fail "tools/list did not declare ${tool}"
done
printf '\nOK: tools/list declared all six tools\n'

step "6. get_milestones answers from the signed record, with citations"
post "$TOKEN" "$WORK_DIR/milestones.body" \
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_milestones","arguments":{}}}' \
    "${SESSION_HEADERS[@]}" || fail "the get_milestones request did not complete"
cat "$WORK_DIR/milestones.body"
printf '\n'

grep -q '"isError":false' "$WORK_DIR/milestones.body" ||
    fail "get_milestones did not answer"
grep -q '"citations"' "$WORK_DIR/milestones.body" ||
    fail "get_milestones returned no citations"
grep -q '"status\\":\\"verified' "$WORK_DIR/milestones.body" ||
    grep -q '"status":"verified"' "$WORK_DIR/milestones.body" ||
    fail "get_milestones cited no document that verified in this call"
if grep -q 'CN-14' "$WORK_DIR/milestones.body"; then
    fail "the buyer's answer named the developer-only contractor note"
fi
printf 'OK: get_milestones answered with citations and no developer-only document\n'

step "7. a buyer is refused another unit, a developer is not"
post "$TOKEN" "$WORK_DIR/payments-refused.body" \
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_payment_status","arguments":{"unit":"0802"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the refused get_payment_status request did not complete"
cat "$WORK_DIR/payments-refused.body"
printf '\n'

grep -q '"isError":true' "$WORK_DIR/payments-refused.body" ||
    fail "a buyer reading another unit was not refused"
grep -q 'role_scope' "$WORK_DIR/payments-refused.body" ||
    fail "the refusal did not name the role scope as the reason"
if grep -q 'BUYER-PV-0802' "$WORK_DIR/payments-refused.body"; then
    fail "the refusal leaked the other unit's data"
fi
printf 'OK: buyer token refused unit 0802\n'

# The developer is a different caller, so it needs a session of its own.
post "$DEVELOPER_TOKEN" "$WORK_DIR/initialize-developer.body" \
    "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"${PROTOCOL_VERSION}\",\"capabilities\":{},\"clientInfo\":{\"name\":\"smoke.sh\",\"version\":\"0.1.0\"}}}" \
    -D "$WORK_DIR/initialize-developer.headers" ||
    fail "the developer initialize request did not complete"
DEVELOPER_SESSION_ID="$(grep -i '^mcp-session-id:' "$WORK_DIR/initialize-developer.headers" |
    cut -d' ' -f2- | tr -d '\r\n')"
DEVELOPER_HEADERS=()
if [ -n "$DEVELOPER_SESSION_ID" ]; then
    DEVELOPER_HEADERS=(-H "Mcp-Session-Id: ${DEVELOPER_SESSION_ID}"
        -H "MCP-Protocol-Version: ${PROTOCOL_VERSION}")
fi

post "$DEVELOPER_TOKEN" "$WORK_DIR/payments-developer.body" \
    '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_payment_status","arguments":{"unit":"0802"}}}' \
    "${DEVELOPER_HEADERS[@]}" ||
    fail "the developer get_payment_status request did not complete"
cat "$WORK_DIR/payments-developer.body"
printf '\n'

grep -q '"isError":false' "$WORK_DIR/payments-developer.body" ||
    fail "the developer token was not given unit 0802"
grep -q 'PS-0802' "$WORK_DIR/payments-developer.body" ||
    fail "the developer answer did not cite unit 0802's signed schedule"
printf 'OK: developer token read unit 0802\n'

step "8. the evidence card is a ui:// resource"
post "$TOKEN" "$WORK_DIR/resources-list.body" \
    '{"jsonrpc":"2.0","id":6,"method":"resources/list"}' \
    "${SESSION_HEADERS[@]}" || fail "the resources/list request did not complete"
head -c 600 "$WORK_DIR/resources-list.body"
printf '\n'

grep -q 'ui://ask-aunova/evidence/' "$WORK_DIR/resources-list.body" ||
    fail "resources/list served no ui:// evidence card"
grep -q 'text/html;profile=mcp-app' "$WORK_DIR/resources-list.body" ||
    fail "the evidence card did not carry the app mime type"
printf 'OK: evidence cards listed under ui://ask-aunova/evidence/\n'

step "9. verify_claim proves one named field, or refuses the arguments"
post "$TOKEN" "$WORK_DIR/claim-true.body" \
    '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"verify_claim","arguments":{"subject_type":"unit","subject_id":"1204","field":"paid_to_date_minor","asserted_value":"147000000"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the true verify_claim request did not complete"
cat "$WORK_DIR/claim-true.body"
printf '\n'
grep -q '"verdict":"SUPPORTED"' "$WORK_DIR/claim-true.body" ||
    fail "the record's own figure for unit 1204 was not SUPPORTED"

post "$TOKEN" "$WORK_DIR/claim-false.body" \
    '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"verify_claim","arguments":{"subject_type":"unit","subject_id":"1204","field":"paid_to_date_minor","asserted_value":"157000000"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the false verify_claim request did not complete"
grep -q '"verdict":"CONTRADICTED"' "$WORK_DIR/claim-false.body" ||
    fail "a figure the record does not state was not CONTRADICTED"

# The whole point of the typed arguments: a figure written any other way is
# refused rather than guessed at.
post "$TOKEN" "$WORK_DIR/claim-unreadable.body" \
    '{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"verify_claim","arguments":{"subject_type":"unit","subject_id":"1204","field":"paid_to_date_minor","asserted_value":"1,470,000.00 AED"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the unreadable verify_claim request did not complete"
grep -q '"verdict":"UNVERIFIED"' "$WORK_DIR/claim-unreadable.body" ||
    fail "an amount this server cannot read exactly was not UNVERIFIED"

# A value outside a closed enum never reaches a verdict at all.
post "$TOKEN" "$WORK_DIR/claim-bad-enum.body" \
    '{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"verify_claim","arguments":{"subject_type":"escrow","subject_id":"ESC-PVT-4471","field":"status","asserted_value":"attested"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the bad-enum verify_claim request did not complete"
cat "$WORK_DIR/claim-bad-enum.body"
printf '\n'
grep -q '"code":-32602' "$WORK_DIR/claim-bad-enum.body" ||
    fail "a subject_type outside the closed enum was not a JSON-RPC invalid-params error"
if grep -q 'verdict' "$WORK_DIR/claim-bad-enum.body"; then
    fail "a schema error must never carry a verdict"
fi
printf 'OK: verify_claim SUPPORTED, CONTRADICTED, UNVERIFIED and a schema error with no verdict\n'

step "10. ask_project states one topic, scoped to the token"
post "$TOKEN" "$WORK_DIR/ask.body" \
    '{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"ask_project","arguments":{"topic":"payments"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the ask_project request did not complete"
cat "$WORK_DIR/ask.body"
printf '\n'
grep -q '"answered":true' "$WORK_DIR/ask.body" ||
    fail "ask_project did not answer about the caller's own unit"
grep -q 'PS-1204' "$WORK_DIR/ask.body" ||
    fail "ask_project cited no signed schedule"
if grep -q 'PS-0802' "$WORK_DIR/ask.body"; then
    fail "a buyer's answer named another unit's schedule"
fi

post "$TOKEN" "$WORK_DIR/ask-other-unit.body" \
    '{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"ask_project","arguments":{"topic":"payments","unit":"0802"}}}' \
    "${SESSION_HEADERS[@]}" || fail "the out-of-scope ask_project request did not complete"
grep -q 'role_scope' "$WORK_DIR/ask-other-unit.body" ||
    fail "a buyer asking about another unit was not refused"
if grep -q '147000000' "$WORK_DIR/ask-other-unit.body"; then
    fail "a refused question was answered about the caller's own unit instead"
fi
printf 'OK: ask_project answered its own unit and refused another\n'

step "11. whats_changed hands back a cursor the client carries"
post "$TOKEN" "$WORK_DIR/changed-first.body" \
    '{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"whats_changed","arguments":{}}}' \
    "${SESSION_HEADERS[@]}" || fail "the first whats_changed request did not complete"
grep -q '"change_id"' "$WORK_DIR/changed-first.body" ||
    fail "a caller with no cursor was told about no changes"
CURSOR="$(grep -o '"next_cursor":"[^"]*"' "$WORK_DIR/changed-first.body" | cut -d'"' -f4)"
[ -n "$CURSOR" ] || fail "whats_changed handed back no cursor"
printf 'cursor %s\n' "$CURSOR"

post "$TOKEN" "$WORK_DIR/changed-second.body" \
    "{\"jsonrpc\":\"2.0\",\"id\":15,\"method\":\"tools/call\",\"params\":{\"name\":\"whats_changed\",\"arguments\":{\"since_cursor\":\"${CURSOR}\"}}}" \
    "${SESSION_HEADERS[@]}" || fail "the second whats_changed request did not complete"
if grep -q '"change_id"' "$WORK_DIR/changed-second.body"; then
    fail "a change the cursor accounts for was reported again"
fi

post "$TOKEN" "$WORK_DIR/changed-nocursor.body" \
    '{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"whats_changed","arguments":{}}}' \
    "${SESSION_HEADERS[@]}" || fail "the third whats_changed request did not complete"
grep -q '"change_id"' "$WORK_DIR/changed-nocursor.body" ||
    fail "a caller who sent no cursor lost a change the server never knew they had"
printf 'OK: the cursor settles what it covers, and nothing is remembered without it\n'

step "12. a request with no Authorization header"
STATUS="$(curl -sS -m 10 \
    -o "$WORK_DIR/unauthorized.body" \
    -D "$WORK_DIR/unauthorized.headers" \
    -w '%{http_code}' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'Content-Type: application/json' \
    --data-binary '{"jsonrpc":"2.0","id":7,"method":"tools/list"}' \
    "$BASE_URL")" || fail "the unauthenticated request did not complete"

printf 'HTTP %s\n' "$STATUS"
cat "$WORK_DIR/unauthorized.body"
printf '\n'
[ "$STATUS" = "401" ] || fail "expected HTTP 401 without a token, got HTTP ${STATUS}"
grep -qi '^www-authenticate:[[:space:]]*Bearer' "$WORK_DIR/unauthorized.headers" ||
    fail "the 401 carried no WWW-Authenticate: Bearer challenge"
if grep -qi 'demo-token' "$WORK_DIR/unauthorized.body"; then
    fail "the 401 body named a token"
fi
printf 'OK: refused with HTTP 401 and a Bearer challenge\n'

step "13. stop the server"
kill "$SERVER_PID"
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""
printf 'server stopped\n'

printf '\nSMOKE OK: protocolVersion %s, six tools declared, cited answers, role refusal, ui:// card, one field proved and one refused, a cursor honoured, 401 without a token\n' \
    "$PROTOCOL_VERSION"
