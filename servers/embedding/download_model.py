"""Download ONNX embedding models for local inference."""

import argparse
import os
import sys
import urllib.request

# MiniLM
MINIML_DIR = os.environ.get("ONNX_MODEL_DIR", "data/models/all-MiniLM-L6-v2")
MINIML_BASE = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"
MINIML_FILES = {
    "model.onnx": f"{MINIML_BASE}/onnx/model.onnx",
    "tokenizer.json": f"{MINIML_BASE}/tokenizer.json",
}

# jina-v5-nano (retrieval variant with merged LoRA)
JINA_BASE = "https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval/resolve/main"
JINA_FILES = {
    "model.onnx": f"{JINA_BASE}/onnx/model.onnx",
    "tokenizer.json": f"{JINA_BASE}/tokenizer.json",
}


def _download_files(model_dir: str, files: dict[str, str]) -> bool:
    os.makedirs(model_dir, exist_ok=True)
    for filename, url in files.items():
        dest = os.path.join(model_dir, filename)
        if os.path.exists(dest):
            print(f"  Already exists: {dest}")
            continue
        print(f"  Downloading: {filename} ...")
        try:
            urllib.request.urlretrieve(url, dest)
            size_mb = os.path.getsize(dest) / (1024 * 1024)
            print(f"  Saved: {dest} ({size_mb:.1f} MB)")
        except Exception as e:
            print(f"  Failed to download {filename}: {e}", file=sys.stderr)
            if os.path.exists(dest):
                os.remove(dest)
            return False
    return True


def download():
    """Download MiniLM (legacy entrypoint)."""
    print("=== Downloading all-MiniLM-L6-v2 ONNX model ===")
    ok = _download_files(MINIML_DIR, MINIML_FILES)
    if ok:
        print(f"Model ready at {MINIML_DIR}")
    return ok


def download_jina_v5_nano(model_dir: str = "") -> bool:
    """Download jina-embeddings-v5-text-nano-retrieval ONNX model."""
    if not model_dir:
        model_dir = os.environ.get("ONNX_MODEL_DIR", "data/models/jina-embeddings-v5-text-nano")
    print("=== Downloading jina-embeddings-v5-text-nano-retrieval ===")
    ok = _download_files(model_dir, JINA_FILES)
    if ok:
        print(f"Model ready at {model_dir}")
    return ok


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Download ONNX embedding models")
    parser.add_argument("--model", default="miniml", choices=["miniml", "jina-v5-nano"])
    args = parser.parse_args()

    if args.model == "miniml":
        success = download()
    else:
        success = download_jina_v5_nano()
    sys.exit(0 if success else 1)
