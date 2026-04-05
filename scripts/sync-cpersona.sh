#!/usr/bin/env bash
# sync-cpersona.sh — Sync CPersona from cloto-mcp-servers to standalone repo
#
# Usage: bash scripts/sync-cpersona.sh [--dry-run]
#
# cloto-mcp-servers is the source of truth.
# The standalone repo (cpersona) is a mirror for standalone users.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MONO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
STANDALONE="${MONO_ROOT}/../cpersona"

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
    echo "[DRY RUN] No files will be modified."
fi

# Validate paths
if [[ ! -d "$MONO_ROOT/servers/cpersona" ]]; then
    echo "ERROR: cloto-mcp-servers/servers/cpersona/ not found" >&2
    exit 1
fi
if [[ ! -d "$STANDALONE" ]]; then
    echo "ERROR: cpersona standalone repo not found at $STANDALONE" >&2
    echo "Expected sibling directory: ../cpersona" >&2
    exit 1
fi

echo "Source: $MONO_ROOT/servers/cpersona/"
echo "Target: $STANDALONE/"
echo ""

# Files to sync (monorepo → standalone)
SYNC_FILES=(
    "server.py"
    "proxy_stdio.py"
    "test_task_queue.py"
)

# Common files to vendor (monorepo common/ → standalone common/)
VENDOR_FILES=(
    "common/mcp_utils.py"
    "common/validation.py"
)

changed=0

sync_file() {
    local src="$1"
    local dst="$2"
    local label="$3"

    if [[ ! -f "$src" ]]; then
        echo "  SKIP  $label (source not found)"
        return
    fi

    if [[ ! -f "$dst" ]] || ! diff -q "$src" "$dst" > /dev/null 2>&1; then
        if $DRY_RUN; then
            echo "  DIFF  $label"
        else
            cp "$src" "$dst"
            echo "  SYNC  $label"
        fi
        changed=$((changed + 1))
    else
        echo "  OK    $label"
    fi
}

echo "=== Server files ==="
for f in "${SYNC_FILES[@]}"; do
    sync_file "$MONO_ROOT/servers/cpersona/$f" "$STANDALONE/$f" "$f"
done

echo ""
echo "=== Vendored common files ==="
for f in "${VENDOR_FILES[@]}"; do
    sync_file "$MONO_ROOT/servers/$f" "$STANDALONE/$f" "$f"
done

echo ""
if [[ $changed -eq 0 ]]; then
    echo "All files are in sync."
else
    if $DRY_RUN; then
        echo "$changed file(s) would be updated. Run without --dry-run to apply."
    else
        echo "$changed file(s) synced."
        echo ""
        echo "Next steps:"
        echo "  cd $STANDALONE"
        echo "  git diff"
        echo "  git add -A && git commit -m 'sync: update from cloto-mcp-servers'"
    fi
fi
