#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
GATEWAY_PID=""
AUTH_GATEWAY_PID=""
FAKE_PID=""
FAKE_BACKUP_PID=""
cargo build --quiet --manifest-path "$ROOT/Cargo.toml"
BIN="$ROOT/target/debug/duola-agentcost"
export HOME="$TMP/home"
mkdir -p "$HOME"

python3 "$ROOT/tests/fake_provider.py" 18080 &
FAKE_PID=$!
python3 "$ROOT/tests/fake_provider.py" 18081 &
FAKE_BACKUP_PID=$!
cleanup() {
  kill "$FAKE_PID" 2>/dev/null || true
  kill "$FAKE_BACKUP_PID" 2>/dev/null || true
  [ -z "$GATEWAY_PID" ] || kill "$GATEWAY_PID" 2>/dev/null || true
  [ -z "$AUTH_GATEWAY_PID" ] || kill "$AUTH_GATEWAY_PID" 2>/dev/null || true
  wait "$FAKE_PID" 2>/dev/null || true
  wait "$FAKE_BACKUP_PID" 2>/dev/null || true
  [ -z "$GATEWAY_PID" ] || wait "$GATEWAY_PID" 2>/dev/null || true
  [ -z "$AUTH_GATEWAY_PID" ] || wait "$AUTH_GATEWAY_PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

"$BIN" install \
  --provider-id primary \
  --endpoint http://127.0.0.1:18080 \
  --protocol openai-responses >/dev/null
"$BIN" provider add primary http://127.0.0.1:18080 --fallback backup --input-price 1 --output-price 2 >/dev/null
"$BIN" provider add backup http://127.0.0.1:18081 --input-price 0.1 --output-price 2 --model-map alias=real >/dev/null

"$BIN" serve \
  --gateway-listen 127.0.0.1:18765 \
  --admin-listen 127.0.0.1:18766 >/tmp/duola-agentcost-e2e.log 2>&1 &
GATEWAY_PID=$!

for _ in $(seq 1 50); do
  curl -fsS http://127.0.0.1:18766/api/status >/dev/null 2>&1 && break
  sleep 0.1
done

curl -fsS http://127.0.0.1:18766/api/rules | python3 -c 'import json,sys; xs=json.load(sys.stdin); assert any(x["id"] == "tool-result.ansi.v1" and x["safety"] == "lossless-text" for x in xs)'
curl -fsS http://127.0.0.1:18766/api/cache/status | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["enabled"] is False; assert x["entries"] == 0'
curl -fsS 'http://127.0.0.1:18766/api/trends?days=30' | python3 -c 'import json,sys; assert isinstance(json.load(sys.stdin), list)'

RESPONSE="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"fake","messages":[{"role":"tool","content":"ok \u001b[31mred\u001b[0m"}]}')"

echo "$RESPONSE" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["ok"] is True; assert x["received"]["messages"][0]["content"] == "ok red"'

BYPASS_RESPONSE="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' -H 'X-DuoLA-Transform: off' \
  -d '{"model":"bypass-header","messages":[{"role":"tool","content":"ok \u001b[31mred\u001b[0m"}]}' )"
echo "$BYPASS_RESPONSE" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert "\\u001b" in json.dumps(x["received"])'

FALLBACK="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"fail","messages":[]}')"
echo "$FALLBACK" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["ok"] is True; assert x["provider_port"] == 18081'

MAPPED="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"alias","messages":[]}')"
echo "$MAPPED" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["provider_port"] == 18081; assert x["received"]["model"] == "real"'

python3 - <<'PY'
import json
import urllib.request

line = "warning: dependency resolver retried request with identical payload 0123456789"
tools = []
for index in range(20):
    tools.append({
        "type": "function",
        "function": {
            "name": f"tool_{index}",
            "description": "search repository files" if index < 5 else "manage unrelated calendar events",
            "parameters": {"type": "object", "properties": {"query": {"type": "string"}}},
        },
    })
payload = {
    "model": "optimization",
    "messages": [
        {"role": "user", "content": "search repository files"},
        {"role": "tool", "content": (line + "\n") * 4},
        {"role": "tool", "content": (line + "\n") * 4},
    ],
    "tools": tools,
}
raw = json.dumps(payload, separators=(",", ":")).encode()
request = urllib.request.Request(
    "http://127.0.0.1:18765/v1/responses",
    data=raw,
    headers={"content-type": "application/json"},
)
with urllib.request.urlopen(request) as response:
    received = json.load(response)["received"]
sent = json.dumps(received, separators=(",", ":")).encode()
assert len(sent) < len(raw), (len(raw), len(sent))
assert "\x1b" not in sent.decode()
assert len(received["tools"]) == 5
assert received["messages"][2]["content"].startswith("[DuoLA] duplicate tool result")
PY

STREAM="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"stream","stream":true,"messages":[]}')"
echo "$STREAM" | grep -q 'response.output_text.delta'

CANCEL_HEADERS="$(curl -sS --max-time 0.3 -D - -o /tmp/duola-agentcost-cancel.out \
  http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"cancel-stream","stream":true,"messages":[]}' || true)"
CANCEL_ID="$(echo "$CANCEL_HEADERS" | awk -F': ' 'tolower($1)=="x-duola-request-id" {gsub("\r", "", $2); print $2}')"
sleep 0.3
CANCEL_STATUS=""
for _ in $(seq 1 30); do
  CANCEL_STATUS="$(curl -fsS http://127.0.0.1:18766/api/requests | python3 -c 'import json,sys; xs=json.load(sys.stdin); target="'"$CANCEL_ID"'"; print(next((x["status"] for x in xs if x["id"] == target), "missing"))')"
  [ "$CANCEL_STATUS" = "cancelled" ] && break
  sleep 0.1
done
[ "$CANCEL_STATUS" = "cancelled" ]

ANTHROPIC_HEADERS="$(curl -sS -D - -o /tmp/duola-agentcost-anthropic-stream.out \
  http://127.0.0.1:18765/v1/messages \
  -H 'content-type: application/json' \
  -d '{"model":"anthropic-stream","stream":true,"messages":[]}')"
ANTHROPIC_ID="$(echo "$ANTHROPIC_HEADERS" | awk -F': ' 'tolower($1)=="x-duola-request-id" {gsub("\r", "", $2); print $2}')"
curl -fsS http://127.0.0.1:18766/api/requests | python3 -c 'import json,sys; x=json.load(sys.stdin); assert any(item["id"] == "'"$ANTHROPIC_ID"'" and item["status"] == "completed" and item["output_tokens"] == 4 for item in x)'

LONG_HEADERS="$(curl -sS -D - -o /tmp/duola-agentcost-long-stream.out \
  http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"long-stream","stream":true,"messages":[]}')"
LONG_ID="$(echo "$LONG_HEADERS" | awk -F': ' 'tolower($1)=="x-duola-request-id" {gsub("\r", "", $2); print $2}')"
curl -fsS "http://127.0.0.1:18766/api/requests/$LONG_ID" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["request_id"].startswith("req_")'
curl -fsS http://127.0.0.1:18766/api/requests | python3 -c 'import json,sys; x=json.load(sys.stdin); assert any(item["id"] == "'"$LONG_ID"'" and item["status"] == "completed" and item["measured_input_tokens"] == 12 for item in x)'

curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' -d '{"model":"repeat","messages":[]}' >/dev/null
curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' -d '{"model":"repeat","messages":[]}' >/dev/null
curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' -d '{"model":"repeat","messages":[]}' >/dev/null
LOOP_STATUS="$(curl -sS -o /tmp/duola-agentcost-loop.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"repeat","messages":[]}')"
[ "$LOOP_STATUS" = "429" ]

"$BIN" bypass >/dev/null
curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' -d '{"model":"repeat","messages":[]}' >/dev/null
"$BIN" restore >/dev/null

curl -fsS -X POST http://127.0.0.1:18766/api/bypass \
  -H 'content-type: application/json' -d '{"enabled":true}' >/dev/null
API_BYPASS_RESPONSE="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"api-bypass","messages":[{"role":"tool","content":"ok \u001b[31mred\u001b[0m"}]}' )"
echo "$API_BYPASS_RESPONSE" | python3 -c 'import json,sys; assert "\\u001b" in json.dumps(json.load(sys.stdin)["received"])'
curl -fsS -X POST http://127.0.0.1:18766/api/bypass \
  -H 'content-type: application/json' -d '{"enabled":false}' >/dev/null
curl -fsS http://127.0.0.1:18766/api/control-events | python3 -c 'import json,sys; xs=json.load(sys.stdin); assert any(x["action"]=="bypass" for x in xs); assert any(x["action"]=="restore" for x in xs)'

"$BIN" cache set --admin-listen 127.0.0.1:18766 --enabled true --ttl-seconds 300 >/dev/null
curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"cache-me","messages":[{"role":"user","content":"read only"}]}' >/dev/null
CACHE_HEADERS="$(curl -sS -D - -o /tmp/duola-agentcost-cache.out \
  http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"cache-me","messages":[{"role":"user","content":"read only"}]}')"
echo "$CACHE_HEADERS" | grep -qi 'x-duola-cache: HIT'
CACHE_ID="$(echo "$CACHE_HEADERS" | awk -F': ' 'tolower($1)=="x-duola-request-id" {gsub("\r", "", $2); print $2}')"
curl -fsS "http://127.0.0.1:18766/api/requests/$CACHE_ID" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["request_id"].startswith("req_"); assert x["attempts"] == []'
curl -fsS http://127.0.0.1:18766/api/cache/status | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["hits"] >= 1; assert x["entries"] >= 1'
"$BIN" cache clear --admin-listen 127.0.0.1:18766 >/dev/null
CLEAR_HEADERS="$(curl -sS -D - -o /tmp/duola-agentcost-cache-clear.out \
  http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"cache-me","messages":[{"role":"user","content":"read only"}]}')"
if echo "$CLEAR_HEADERS" | grep -qi 'x-duola-cache: HIT'; then
  echo "cache clear failed" >&2
  exit 1
fi

"$BIN" privacy set --admin-listen 127.0.0.1:18766 --strict >/dev/null
STRICT_CACHE_HEADERS="$(curl -sS -D - -o /tmp/duola-agentcost-strict-cache.out \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"strict-cache","messages":[{"role":"user","content":"read only"}]}' )"
if echo "$STRICT_CACHE_HEADERS" | grep -qi 'x-duola-cache: HIT'; then
  echo "strict privacy unexpectedly served cache" >&2
  exit 1
fi
"$BIN" privacy set --admin-listen 127.0.0.1:18766 --relaxed >/dev/null

"$BIN" budget set --admin-listen 127.0.0.1:18766 --request-output-tokens 7 >/dev/null
OUTPUT_CAP="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"output-cap","messages":[]}' )"
echo "$OUTPUT_CAP" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["received"]["max_output_tokens"] == 7'

"$BIN" routing set --admin-listen 127.0.0.1:18766 --mode cost --max-attempts 3 >/dev/null
COST_ROUTE="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"cost-route","messages":[]}' )"
echo "$COST_ROUTE" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["provider_port"] == 18081'

# Task aggregation and explicit scoped budgets use only request metadata; no
# prompt text is used to infer a project or a budget.
TASK_RESPONSE="$(curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -H 'X-DuoLA-Agent-Session: task-e2e' \
  -H 'X-DuoLA-Project: backend' \
  -H 'X-DuoLA-Agent: fake-agent' \
  -d '{"model":"task-model","messages":[{"role":"user","content":"task aggregation"}]}' )"
echo "$TASK_RESPONSE" | python3 -c 'import json,sys; assert json.load(sys.stdin)["ok"] is True'
curl -fsS http://127.0.0.1:18766/api/tasks | python3 -c 'import json,sys; xs=json.load(sys.stdin); x=next(x for x in xs if x["session_id"]=="task-e2e"); assert x["project_id"]=="backend"; assert x["agent"]=="fake-agent"; assert x["requests"]>=1'

"$BIN" budget set --admin-listen 127.0.0.1:18766 --scope project:limited --request-tokens 1 >/dev/null
SCOPED_STATUS="$(curl -sS -o /tmp/duola-agentcost-scoped-budget.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -H 'X-DuoLA-Project: limited' -d '{"model":"scoped","messages":[{"role":"user","content":"this is over one token"}]}' )"
[ "$SCOPED_STATUS" = "429" ]
UNSCOPED_STATUS="$(curl -sS -o /tmp/duola-agentcost-unscoped-budget.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"unscoped","messages":[]}' )"
[ "$UNSCOPED_STATUS" = "200" ]

mkdir -p "$HOME/.codex" "$TMP/bin"
cp "$ROOT/tests/codex.config.toml" "$HOME/.codex/config.toml"
cp "$ROOT/tests/fake_codex.sh" "$TMP/bin/codex"
chmod +x "$TMP/bin/codex"
PATH="$TMP/bin:$PATH" "$BIN" launch codex >/dev/null
grep -q 'model_provider = "original"' "$HOME/.codex/config.toml"

STATS="$(curl -fsS http://127.0.0.1:18766/api/stats)"
echo "$STATS" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["requests"] >= 12; assert x["blocked_requests"] >= 1; assert x["cache_hit_requests"] >= 1; assert x["cache_saved_input_tokens"] > 0; assert x["applied_rules"] >= 5; assert x["total_cost"] > 0; assert x["successful_requests"] >= 10; assert x["failed_requests"] >= 1; assert x["measured_requests"] >= 9; assert x["transformed_requests"] >= 3; assert x["saved_input_tokens"] > 0'

REQUESTS="$(curl -fsS http://127.0.0.1:18766/api/requests)"
DETAIL_ID="$(echo "$REQUESTS" | python3 -c 'import json,sys,urllib.request; xs=json.load(sys.stdin); print(next(x["id"] for x in xs if x["status"] not in ("cache_hit","budget_blocked","loop_blocked") and len(json.load(urllib.request.urlopen("http://127.0.0.1:18766/api/requests/" + x["id"]))["attempts"]) == 2))')"
DETAIL="$(curl -fsS "http://127.0.0.1:18766/api/requests/$DETAIL_ID")"
echo "$DETAIL" | python3 -c 'import json,sys; x=json.load(sys.stdin); assert len(x["attempts"]) == 2; assert x["attempts"][0]["status"] == "transient"; assert x["attempts"][1]["status"] == "received"'

"$BIN" budget set --admin-listen 127.0.0.1:18766 --request-usd 0.000001 >/dev/null
curl -fsS http://127.0.0.1:18766/api/status | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["request_budget_usd"] == 0.000001'
BUDGET_STATUS="$(curl -sS -o /tmp/duola-agentcost-budget.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"budget","messages":[]}')"
[ "$BUDGET_STATUS" = "429" ]

"$BIN" budget set --admin-listen 127.0.0.1:18766 --request-usd 100 >/dev/null
curl -fsS http://127.0.0.1:18765/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"budget-cache","messages":[]}' >/dev/null

"$BIN" budget set --admin-listen 127.0.0.1:18766 --request-usd 100 --request-tokens 1 >/dev/null
curl -fsS http://127.0.0.1:18766/api/status | python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["request_token_budget"] == 1'
TOKEN_STATUS="$(curl -sS -o /tmp/duola-agentcost-token-budget.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"token-budget","messages":[{"role":"user","content":"this request must be blocked"}]}' )"
[ "$TOKEN_STATUS" = "429" ]

"$BIN" budget set --admin-listen 127.0.0.1:18766 --request-usd 100 --request-tokens 100 --session-tokens 20 >/dev/null
curl -sS --max-time 2 -o /tmp/duola-agentcost-token-reservation.out \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"cancel-stream","stream":true,"messages":[]}' >/dev/null 2>&1 &
TOKEN_RESERVATION_PID=$!
sleep 0.2
TOKEN_RESERVATION_STATUS="$(curl -sS -o /tmp/duola-agentcost-token-reservation-blocked.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"cancel-stream","stream":true,"messages":[]}' || true)"
[ "$TOKEN_RESERVATION_STATUS" = "429" ]
wait "$TOKEN_RESERVATION_PID" 2>/dev/null || true

CACHE_BUDGET_STATUS="$(curl -sS -o /tmp/duola-agentcost-cache-budget.out -w '%{http_code}' \
  http://127.0.0.1:18765/v1/responses -H 'content-type: application/json' \
  -d '{"model":"budget-cache","messages":[]}' )"
[ "$CACHE_BUDGET_STATUS" = "429" ]

AUTH_CONFIG="$TMP/auth.toml"
DUOLA_E2E_GATEWAY_TOKEN="e2e-secret" "$BIN" install --config "$AUTH_CONFIG" \
  --provider-id primary --endpoint http://127.0.0.1:18080 --protocol openai-responses >/dev/null
DUOLA_E2E_GATEWAY_TOKEN="e2e-secret" "$BIN" provider add --config "$AUTH_CONFIG" primary \
  http://127.0.0.1:18080 --input-price 1 --output-price 2 >/dev/null
python3 - "$AUTH_CONFIG" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
lines = path.read_text().splitlines()
lines[0] = 'gateway_listen = "0.0.0.0:18767"'
lines[1] = 'admin_listen = "127.0.0.1:18768"'
lines.insert(2, 'gateway_auth_token_env = "DUOLA_E2E_GATEWAY_TOKEN"')
path.write_text("\n".join(lines) + "\n")
PY
DUOLA_E2E_GATEWAY_TOKEN="e2e-secret" "$BIN" serve --config "$AUTH_CONFIG" \
  >/tmp/duola-agentcost-auth-e2e.log 2>&1 &
AUTH_GATEWAY_PID=$!
for _ in $(seq 1 50); do
  curl -fsS http://127.0.0.1:18768/healthz >/dev/null 2>&1 && break
  sleep 0.1
done
AUTH_STATUS="$(curl -sS -o /tmp/duola-agentcost-auth.out -w '%{http_code}' \
  http://127.0.0.1:18767/v1/responses -H 'content-type: application/json' \
  -d '{"model":"auth","messages":[]}')"
[ "$AUTH_STATUS" = "401" ]
AUTH_RESPONSE="$(curl -fsS http://127.0.0.1:18767/v1/responses \
  -H 'content-type: application/json' -H 'X-DuoLA-Gateway-Token: e2e-secret' \
  -d '{"model":"auth","messages":[]}')"
echo "$AUTH_RESPONSE" | python3 -c 'import json,sys; assert json.load(sys.stdin)["ok"] is True'

EXPORT_PATH="$TMP/ledger.json"
"$BIN" export "$EXPORT_PATH" >/dev/null
python3 - "$EXPORT_PATH" <<'PY'
import json, sys
records = json.load(open(sys.argv[1]))
assert records
assert all("messages" not in record and "body" not in record for record in records)
PY
CSV_EXPORT_PATH="$TMP/ledger.csv"
"$BIN" export "$CSV_EXPORT_PATH" --format csv >/dev/null
head -n 1 "$CSV_EXPORT_PATH" | grep -q '^id,provider,path,status,'
"$BIN" data purge --older-than-days 1 >/dev/null

kill -TERM "$AUTH_GATEWAY_PID"
wait "$AUTH_GATEWAY_PID"
AUTH_GATEWAY_PID=""

echo "E2E PASS"
