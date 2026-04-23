"""Tests for _select_ort_providers() in embedding/server.py.

All tests mock onnxruntime.get_available_providers() via unittest.mock.patch
and re-load the embedding server module per-test (importlib.util pattern,
matching test_embedding_index.py) so module-level env parsing is refreshed.

No real ONNX inference. Runs on ubuntu-latest CI (no GPU/CoreML/DirectML).
"""

import importlib.util
import os
from unittest.mock import patch

import pytest  # noqa: F401 — imported to mirror test_embedding_config.py style

_SERVER_PATH = os.path.join(os.path.dirname(__file__), "..", "embedding", "server.py")


def _load_embedding_server_module():
    """Load embedding/server.py as a fresh module object.

    Using spec_from_file_location avoids polluting the sys.modules cache and
    prevents name collisions with cpersona.server.
    """
    spec = importlib.util.spec_from_file_location("embedding_server_test", _SERVER_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_auto_detect_coreml_on_mac(monkeypatch):
    """Auto-mode on macOS prioritizes CoreML, then CPU."""
    monkeypatch.delenv("ONNX_EP_PREFERENCE", raising=False)
    with patch(
        "onnxruntime.get_available_providers",
        return_value=["CoreMLExecutionProvider", "CPUExecutionProvider"],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers == ["CoreMLExecutionProvider", "CPUExecutionProvider"]


def test_auto_detect_directml_on_windows(monkeypatch):
    """Auto-mode on Windows preserves DirectML behavior (regression guard)."""
    monkeypatch.delenv("ONNX_EP_PREFERENCE", raising=False)
    with patch(
        "onnxruntime.get_available_providers",
        return_value=["DmlExecutionProvider", "CPUExecutionProvider"],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers == ["DmlExecutionProvider", "CPUExecutionProvider"]


def test_auto_detect_cpu_only_on_linux(monkeypatch):
    """On plain Linux (no accelerator), CPU-only path returns [CPU]."""
    monkeypatch.delenv("ONNX_EP_PREFERENCE", raising=False)
    with patch(
        "onnxruntime.get_available_providers",
        return_value=["CPUExecutionProvider"],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers == ["CPUExecutionProvider"]


def test_explicit_preference_respected(monkeypatch):
    """ONNX_EP_PREFERENCE overrides auto-detection for available EPs."""
    monkeypatch.setenv("ONNX_EP_PREFERENCE", "CoreMLExecutionProvider,CPUExecutionProvider")
    with patch(
        "onnxruntime.get_available_providers",
        return_value=[
            "CoreMLExecutionProvider",
            "DmlExecutionProvider",
            "CPUExecutionProvider",
        ],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers == ["CoreMLExecutionProvider", "CPUExecutionProvider"]
    assert "DmlExecutionProvider" not in providers


def test_unknown_ep_silently_dropped(monkeypatch):
    """Unknown/unavailable EPs are silently dropped; CPU remains the floor."""
    monkeypatch.setenv("ONNX_EP_PREFERENCE", "CUDAExecutionProvider,TensorrtExecutionProvider")
    with patch(
        "onnxruntime.get_available_providers",
        return_value=["CPUExecutionProvider"],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers == ["CPUExecutionProvider"]


def test_cpu_always_appended_even_if_not_requested(monkeypatch):
    """User requesting only CoreML still gets CPU appended as safety fallback."""
    monkeypatch.setenv("ONNX_EP_PREFERENCE", "CoreMLExecutionProvider")
    with patch(
        "onnxruntime.get_available_providers",
        return_value=["CoreMLExecutionProvider", "CPUExecutionProvider"],
    ):
        mod = _load_embedding_server_module()
        providers = mod._select_ort_providers()
    assert providers[0] == "CoreMLExecutionProvider"
    assert providers[-1] == "CPUExecutionProvider"
    assert len(providers) == 2
