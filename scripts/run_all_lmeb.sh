#!/bin/bash
# Run LMEB benchmark for all local embedding models sequentially.
# Usage: bash scripts/run_all_lmeb.sh
#
# Each model's results are cached — if a task is already complete, it's skipped.
# Safe to interrupt and restart.

set -e
cd "$(dirname "$0")/.."
export PYTHONIOENCODING=utf-8

MODELS=(
    "sentence-transformers/all-MiniLM-L6-v2|64"
    "intfloat/multilingual-e5-small|64"
    "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2|64"
    "BAAI/bge-m3|16"
)

for entry in "${MODELS[@]}"; do
    IFS='|' read -r model_path batch_size <<< "$entry"
    model_name=$(basename "$model_path")
    output_dir="lmeb_results/${model_name}"

    echo ""
    echo "================================================================"
    echo "Model: ${model_path} (batch_size=${batch_size})"
    echo "Output: ${output_dir}"
    echo "Started: $(date)"
    echo "================================================================"

    python scripts/benchmark_lmeb.py \
        --model_path "$model_path" \
        --output_dir "$output_dir" \
        --batch_size "$batch_size" \
        2>&1 | tee "${output_dir}_run.log"

    echo "Completed: $(date)"
done

echo ""
echo "================================================================"
echo "All models complete. Running summary..."
echo "================================================================"
python scripts/benchmark_lmeb_summary.py
