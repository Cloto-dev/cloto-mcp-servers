#!/bin/bash
export LD_PRELOAD=/opt/rocm-7.2.0/lib/libamdhip64.so
export HSA_OVERRIDE_GFX_VERSION=11.0.0
export HSA_ENABLE_SDMA=0
export LD_LIBRARY_PATH=/opt/rocm/lib:/opt/rocm/lib64:$LD_LIBRARY_PATH
export PATH=$PATH:/opt/rocm/bin
export LMEB_DIR=/mnt/c/Users/Cycia/AppData/Local/Temp/lmeb

cd /mnt/c/Users/Cycia/source/repos/cloto-mcp-servers

python3 scripts/benchmark_trackb_lmeb.py \
    --model_path google/embeddinggemma-300m \
    --device cuda \
    --recall_mode rrf \
    --auto_calibrate \
    --output_dir trackb_results_gemma300m \
    2>&1 | tee /tmp/benchmark_gemma300m.log
