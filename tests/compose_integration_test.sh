#!/usr/bin/env bash
# Compose-level integration tests for jimini-normalizer
# Exercises all API endpoints against a running Docker Compose service.
#
# Usage:
#   ./tests/compose_integration_test.sh          # expects service already running on :8080
#   COMPOSE_UP=1 ./tests/compose_integration_test.sh  # starts compose, runs tests, tears down

set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
PASS=0
FAIL=0

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

cleanup() {
    if [[ "${COMPOSE_UP:-}" == "1" ]]; then
        echo -e "\n${YELLOW}Tearing down compose...${NC}"
        docker compose down --remove-orphans 2>/dev/null || true
    fi
}
trap cleanup EXIT

if [[ "${COMPOSE_UP:-}" == "1" ]]; then
    echo -e "${YELLOW}Starting compose services...${NC}"
    docker compose up -d --build --wait 2>&1
    echo -e "${GREEN}Services ready.${NC}\n"
fi

assert_status() {
    local test_name="$1"
    local expected="$2"
    local actual="$3"

    if [[ "$actual" == "$expected" ]]; then
        echo -e "  ${GREEN}PASS${NC} $test_name (HTTP $actual)"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $test_name — expected HTTP $expected, got $actual"
        FAIL=$((FAIL + 1))
    fi
}

assert_json() {
    local test_name="$1"
    local jq_expr="$2"
    local expected="$3"
    local body="$4"

    local actual
    actual=$(echo "$body" | jq -r "$jq_expr" 2>/dev/null || echo "__JQ_ERROR__")

    # Normalize numeric comparisons: strip trailing .0 for integer-like floats
    local norm_actual norm_expected
    norm_actual=$(echo "$actual" | sed 's/\.0$//')
    norm_expected=$(echo "$expected" | sed 's/\.0$//')

    if [[ "$norm_actual" == "$norm_expected" ]]; then
        echo -e "  ${GREEN}PASS${NC} $test_name"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $test_name — expected '$expected', got '$actual'"
        FAIL=$((FAIL + 1))
    fi
}

assert_contains() {
    local test_name="$1"
    local needle="$2"
    local body="$3"

    if echo "$body" | grep -q "$needle"; then
        echo -e "  ${GREEN}PASS${NC} $test_name"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $test_name — body does not contain '$needle'"
        FAIL=$((FAIL + 1))
    fi
}

assert_not_contains() {
    local test_name="$1"
    local needle="$2"
    local body="$3"

    if ! echo "$body" | grep -q "$needle"; then
        echo -e "  ${GREEN}PASS${NC} $test_name"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $test_name — body should NOT contain '$needle'"
        FAIL=$((FAIL + 1))
    fi
}

# ── 1. Health check ──────────────────────────────────────────────────

echo "1. Health endpoint"
RESP=$(curl -s -w "\n%{http_code}" "$BASE_URL/health")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "GET /health returns 200" "200" "$STATUS"
assert_json "health status is ok" ".status" "ok" "$BODY"

# ── 2. Provider A — nested JSON with score scaling ───────────────────

echo -e "\n2. Normalize Provider A (nested JSON, 0-10 scores)"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=provider_a" \
    -H "Content-Type: application/json" \
    -d @fixtures/provider_a.json)
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "POST /normalize?provider=provider_a returns 200" "200" "$STATUS"
assert_json "returns array of 1 assessment" ". | length" "1" "$BODY"
assert_json "patientId is P123" ".[0].patientId" "P123" "$BODY"
assert_json "assessmentType is behavioral_screening" ".[0].assessmentType" "behavioral_screening" "$BODY"
assert_json "sourceProvider is provider_a" ".[0].metadata.sourceProvider" "provider_a" "$BODY"
assert_json "sourceFormat is nested_json" ".[0].metadata.sourceFormat" "nested_json" "$BODY"
assert_json "has 3 scores" ".[0].scores | length" "3" "$BODY"

# Score scaling: 0-10 → 0-100
assert_json "anxiety score scaled to 70" ".[0].scores[] | select(.dimension == \"anxiety\") | .value" "70" "$BODY"
assert_json "social score scaled to 40" ".[0].scores[] | select(.dimension == \"social\") | .value" "40" "$BODY"
assert_json "attention score scaled to 60" ".[0].scores[] | select(.dimension == \"attention\") | .value" "60" "$BODY"
assert_json "score scale is 0-100" ".[0].scores[0].scale" "0-100" "$BODY"

# PHI redaction — name and DOB must NOT appear in output
assert_not_contains "PHI: patient name not in output" "Jane Doe" "$BODY"
assert_not_contains "PHI: DOB not in output" "1990-05-15" "$BODY"

# camelCase output verification
assert_contains "output uses camelCase patientId" "patientId" "$BODY"
assert_contains "output uses camelCase assessmentType" "assessmentType" "$BODY"
assert_contains "output uses camelCase sourceProvider" "sourceProvider" "$BODY"

# ── 3. Provider B — flat KV with score_* prefix ─────────────────────

echo -e "\n3. Normalize Provider B (flat KV, score_* keys)"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=provider_b" \
    -H "Content-Type: application/json" \
    -d @fixtures/provider_b.json)
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "POST /normalize?provider=provider_b returns 200" "200" "$STATUS"
assert_json "patientId is P123" ".[0].patientId" "P123" "$BODY"
assert_json "assessmentType is cognitive" ".[0].assessmentType" "cognitive" "$BODY"
assert_json "sourceFormat is flat_kv" ".[0].metadata.sourceFormat" "flat_kv" "$BODY"
assert_json "has 2 scores (memory, processing)" ".[0].scores | length" "2" "$BODY"
assert_json "memory score is 85 (no scaling)" ".[0].scores[] | select(.dimension == \"memory\") | .value" "85" "$BODY"
assert_json "processing score is 72 (no scaling)" ".[0].scores[] | select(.dimension == \"processing\") | .value" "72" "$BODY"

# PHI
assert_not_contains "PHI: patient_name not in output" "Jane Doe" "$BODY"

# ── 4. Provider C — CSV with row grouping ────────────────────────────

echo -e "\n4. Normalize Provider C (CSV, row grouping)"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=provider_c" \
    -H "Content-Type: text/csv" \
    --data-binary @fixtures/provider_c.csv)
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "POST /normalize?provider=provider_c returns 200" "200" "$STATUS"
assert_json "returns 1 assessment (rows grouped)" ". | length" "1" "$BODY"
assert_json "patientId is P123" ".[0].patientId" "P123" "$BODY"
assert_json "assessmentType is behavioral" ".[0].assessmentType" "behavioral" "$BODY"
assert_json "has 2 scores" ".[0].scores | length" "2" "$BODY"

# Score scaling: 0-10 → 0-100
assert_json "attention_span scaled to 60" ".[0].scores[] | select(.dimension == \"attention_span\") | .value" "60" "$BODY"
assert_json "social_engagement scaled to 40" ".[0].scores[] | select(.dimension == \"social_engagement\") | .value" "40" "$BODY"

# ── 5. Unknown provider — 400 ───────────────────────────────────────

echo -e "\n5. Unknown provider returns 400"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=unknown" \
    -H "Content-Type: application/json" \
    -d '{}')
STATUS=$(echo "$RESP" | tail -1)

assert_status "POST /normalize?provider=unknown returns 400" "400" "$STATUS"

# ── 6. Malformed input — error handling ──────────────────────────────

echo -e "\n6. Malformed input returns error"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=provider_a" \
    -H "Content-Type: application/json" \
    -d 'not json at all')
STATUS=$(echo "$RESP" | tail -1)

# Should return 400 or 422
if [[ "$STATUS" == "400" || "$STATUS" == "422" ]]; then
    echo -e "  ${GREEN}PASS${NC} malformed input returns HTTP $STATUS"
    PASS=$((PASS + 1))
else
    echo -e "  ${RED}FAIL${NC} malformed input — expected 400 or 422, got $STATUS"
    FAIL=$((FAIL + 1))
fi

# ── 7. Round-trip: normalize → retrieve by ID ────────────────────────

echo -e "\n7. Assessment round-trip (normalize → GET by ID)"
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize?provider=provider_a" \
    -H "Content-Type: application/json" \
    -d @fixtures/provider_a.json)
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "normalize for round-trip returns 200" "200" "$STATUS"
ID=$(echo "$BODY" | jq -r '.[0].id')

RESP2=$(curl -s -w "\n%{http_code}" "$BASE_URL/assessments/$ID")
STATUS2=$(echo "$RESP2" | tail -1)
BODY2=$(echo "$RESP2" | sed '$d')

assert_status "GET /assessments/:id returns 200" "200" "$STATUS2"
assert_json "retrieved assessment has same ID" ".id" "$ID" "$BODY2"
assert_json "retrieved assessment has correct patientId" ".patientId" "P123" "$BODY2"

# ── 8. GET /assessments — list ───────────────────────────────────────

echo -e "\n8. List assessments"
RESP=$(curl -s -w "\n%{http_code}" "$BASE_URL/assessments")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "GET /assessments returns 200" "200" "$STATUS"
COUNT=$(echo "$BODY" | jq '. | length')
if [[ "$COUNT" -gt 0 ]]; then
    echo -e "  ${GREEN}PASS${NC} assessments list is non-empty ($COUNT items)"
    PASS=$((PASS + 1))
else
    echo -e "  ${RED}FAIL${NC} assessments list is empty"
    FAIL=$((FAIL + 1))
fi

# ── 9. GET /assessments/:bad-id — 404 ───────────────────────────────

echo -e "\n9. Non-existent assessment returns 404"
RESP=$(curl -s -w "\n%{http_code}" "$BASE_URL/assessments/00000000-0000-0000-0000-000000000000")
STATUS=$(echo "$RESP" | tail -1)

assert_status "GET /assessments/bad-id returns 404" "404" "$STATUS"

# ── 10. Batch normalize with partial failure ─────────────────────────

echo -e "\n10. Batch normalize with partial failure (207)"
BATCH='[
  {"provider":"provider_b","data":{"patient_id":"P456","assessment_type":"cognitive","score_memory":90}},
  {"provider":"unknown_provider","data":{}}
]'
RESP=$(curl -s -w "\n%{http_code}" \
    -X POST "$BASE_URL/normalize/batch" \
    -H "Content-Type: application/json" \
    -d "$BATCH")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "POST /normalize/batch returns 207" "207" "$STATUS"
assert_json "batch has 1 success" ".successes | length" "1" "$BODY"
assert_json "batch has 1 error" ".errors | length" "1" "$BODY"

# ── 11. Metrics endpoint ────────────────────────────────────────────

echo -e "\n11. Metrics endpoint (Prometheus format)"
RESP=$(curl -s -w "\n%{http_code}" "$BASE_URL/metrics")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

assert_status "GET /metrics returns 200" "200" "$STATUS"
assert_contains "metrics include http_requests_total" "jimini_http_requests_total" "$BODY"
assert_contains "metrics include normalizations_total" "jimini_normalizations_total" "$BODY"

# ── Summary ──────────────────────────────────────────────────────────

TOTAL=$((PASS + FAIL))
echo -e "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, $TOTAL total"
echo -e "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
