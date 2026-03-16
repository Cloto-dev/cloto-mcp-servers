#!/usr/bin/env bash
# Sentinel — automated test quality guard (Python/pytest)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ERRORS=0

# ── 1. Check for test file deletions ──────────────────────────
if git rev-parse HEAD~1 >/dev/null 2>&1; then
    DELETED_TEST_FILES=$(git diff --name-only --diff-filter=D HEAD~1 2>/dev/null \
        | grep -E 'test_.*\.py$|.*_test\.py$' || true)
    if [ -n "$DELETED_TEST_FILES" ]; then
        echo "SENTINEL: Test files deleted:"
        echo "$DELETED_TEST_FILES" | while read -r f; do echo "   - $f"; done
        ERRORS=$((ERRORS + 1))
    fi
fi

# ── 2. Check for assertion-less test functions ────────────────
TEST_FILES=$(find "$PROJECT_ROOT/servers/tests/" -name 'test_*.py' 2>/dev/null || true)
for f in $TEST_FILES; do
    TEST_COUNT=$(grep -c 'def test_' "$f" 2>/dev/null) || TEST_COUNT=0
    ASSERTIONS=$(grep -cE 'assert |assert\(|pytest\.raises|mock\.|MagicMock|AsyncMock' "$f" 2>/dev/null) || ASSERTIONS=0
    if [ "$TEST_COUNT" -gt 0 ] && [ "$ASSERTIONS" -eq 0 ]; then
        echo "SENTINEL: $f has $TEST_COUNT test(s) but no assertions"
        ERRORS=$((ERRORS + 1))
    fi
done

# ── 3. Check for large test deletions vs additions ────────────
if git rev-parse HEAD~1 >/dev/null 2>&1; then
    TEST_ADDITIONS=$(git diff --numstat HEAD~1 -- '*.py' 2>/dev/null \
        | { grep -E 'test_|_test\.|/tests/' || true; } \
        | awk '{sum += $1} END {print sum+0}')
    TEST_DELETIONS=$(git diff --numstat HEAD~1 -- '*.py' 2>/dev/null \
        | { grep -E 'test_|_test\.|/tests/' || true; } \
        | awk '{sum += $2} END {print sum+0}')

    if [ "$TEST_DELETIONS" -gt 0 ] && [ "$TEST_DELETIONS" -gt "$((TEST_ADDITIONS * 2))" ]; then
        echo "SENTINEL: Test deletions ($TEST_DELETIONS lines) far exceed additions ($TEST_ADDITIONS lines)"
        ERRORS=$((ERRORS + 1))
    fi
fi

# ── 4. Run issue registry verification ────────────────────────
echo "--- Issue Registry Verification ---"
bash "$SCRIPT_DIR/verify-issues.sh" 2>&1 | tail -8
VERIFY_EXIT=${PIPESTATUS[0]:-0}
if [ "$VERIFY_EXIT" -ne 0 ]; then
    ERRORS=$((ERRORS + 1))
fi

# ── Summary ───────────────────────────────────────────────────
if [ "$ERRORS" -gt 0 ]; then
    echo ""
    echo "SENTINEL: $ERRORS violation(s) detected."
    exit 1
fi

echo "Sentinel passed -- no quality regressions detected"
