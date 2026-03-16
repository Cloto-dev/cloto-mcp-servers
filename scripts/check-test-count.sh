#!/usr/bin/env bash
# Test Count Ratchet — blocks CI if test count decreases
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$PROJECT_ROOT/qa/test-baseline.json"

# Detect python command
PYTHON_CMD="python3"
if ! "$PYTHON_CMD" -c "pass" 2>/dev/null; then
    PYTHON_CMD="python"
fi

# 1. Collect pytest test count
PYTEST_OUTPUT=$(cd "$PROJECT_ROOT/servers" && $PYTHON_CMD -m pytest tests/ --collect-only -q 2>/dev/null || true)
PYTEST_COUNT=$(echo "$PYTEST_OUTPUT" | tail -1 | sed -n 's/^\([0-9]*\) tests\{0,1\} collected.*/\1/p')

if [ -z "$PYTEST_COUNT" ]; then
    echo "ERROR: Could not determine test count from pytest output"
    echo "Output was: $PYTEST_OUTPUT"
    exit 1
fi

# 2. Read baseline
_BASELINE_PY="$BASELINE_FILE"
if command -v cygpath &>/dev/null; then
    _BASELINE_PY="$(cygpath -m "$BASELINE_FILE")"
fi
BASELINE=$($PYTHON_CMD -c "import json; print(json.load(open('$_BASELINE_PY'))['pytest_test_count'])")

# 3. Compare
echo "pytest tests: ${PYTEST_COUNT} (baseline: ${BASELINE})"

if [ "$PYTEST_COUNT" -lt "$BASELINE" ]; then
    echo "RATCHET FAILED: Test count decreased (${PYTEST_COUNT} < ${BASELINE})"
    echo "   If tests were intentionally removed, update qa/test-baseline.json"
    exit 1
fi

if [ "$PYTEST_COUNT" -gt "$BASELINE" ]; then
    echo "Test count increased! Consider updating baseline: ${PYTEST_COUNT}"
fi

echo "Test count ratchet passed"
