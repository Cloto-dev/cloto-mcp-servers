#!/usr/bin/env python3
"""
CPersona arm switcher for multi-version comparative benchmark.

Updates memory.cpersona env vars in ClotoCore DB, kills the running
CPersona process, and waits for ClotoCore to auto-restart it.

Usage:
    python3 cpersona_arm_switch.py v2412   # simulate v2.4.12 behaviour
    python3 cpersona_arm_switch.py v2413   # simulate v2.4.13 behaviour
    python3 cpersona_arm_switch.py v2414   # current (all defaults)
    python3 cpersona_arm_switch.py list    # show all arms
"""

import argparse
import json
import os
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.request

CLOTOCORE_DB  = "/Users/hachiya/Desktop/repos/ClotoCore/target/debug/data/cloto_memories.db"
CPERSONA_DB   = "/Users/hachiya/Desktop/repos/ClotoCore/dashboard/src-tauri/cpersona.db"
EMBED_URL     = "http://127.0.0.1:8401/embed"
API_BASE      = "http://127.0.0.1:8081/api"
API_KEY       = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"

BASE_ENV = {
    "CPERSONA_DB_PATH":       "./cpersona.db",
    "CPERSONA_EMBEDDING_MODE": "http",
    "CPERSONA_EMBEDDING_URL":  "http://127.0.0.1:8401/embed",
}

ARMS = {
    "v2412": {
        "label": "v2.4.12 baseline (chat-turn, no AUTOCUT, no episode penalty)",
        "env": {
            **BASE_ENV,
            "CLOTO_MEMORY_INJECTION":          "chat",
            "CPERSONA_AUTOCUT_ENABLED":         "false",
            "CPERSONA_EPISODE_PENALTY_ENABLED": "false",
        },
    },
    "v2413": {
        "label": "v2.4.13 AUTOCUT + XML fence (no episode penalty)",
        "env": {
            **BASE_ENV,
            # XML fence is default (xml_user_prefix), AUTOCUT default true
            "CPERSONA_EPISODE_PENALTY_ENABLED": "false",
        },
    },
    "v2414": {
        "label": "v2.4.14 — XML fence + AUTOCUT + episode penalty (global threshold)",
        "env": {
            **BASE_ENV,
            # all defaults: xml_user_prefix, autocut=true, episode_penalty=true
            # per-agent threshold NOT active (AUTO_CALIBRATE=false default)
        },
    },
    "v2415": {
        "label": "v2.4.15 — + per-agent threshold via CPERSONA_AUTO_CALIBRATE=true",
        "env": {
            **BASE_ENV,
            # all v2414 defaults plus auto-calibrate on startup
            # CPersona computes per-agent threshold for each agent from their corpus
            "CPERSONA_AUTO_CALIBRATE": "true",
        },
    },
}


def _find_cpersona_pid():
    try:
        result = subprocess.run(
            ["pgrep", "-f", "mcp-servers/cpersona/server.py"],
            capture_output=True, text=True
        )
        pids = [int(p) for p in result.stdout.strip().split() if p]
        return pids[0] if pids else None
    except Exception:
        return None


def _wait_cpersona_restart(old_pid, timeout: float = 30.0) -> bool:
    """Wait until a new CPersona process is running (different PID)."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        new_pid = _find_cpersona_pid()
        if new_pid and new_pid != old_pid:
            return True
        time.sleep(1)
    return False


def _update_db_env(arm_key: str):
    env_json = json.dumps(ARMS[arm_key]["env"])
    db = sqlite3.connect(CLOTOCORE_DB)
    try:
        db.execute(
            "UPDATE mcp_servers SET env=? WHERE name='memory.cpersona'",
            (env_json,)
        )
        db.commit()
        print(f"  DB updated: memory.cpersona env → {arm_key}")
    finally:
        db.close()


def _restart_cpersona():
    old_pid = _find_cpersona_pid()
    if old_pid:
        print(f"  Killing CPersona pid={old_pid}...", end=" ", flush=True)
        try:
            os.kill(old_pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        if _wait_cpersona_restart(old_pid, timeout=20):
            new_pid = _find_cpersona_pid()
            print(f"restarted (pid={new_pid})")
            time.sleep(2)  # let server initialize
            return True
        else:
            print("WARNING: CPersona did not restart within 20s")
            return False
    else:
        print("  CPersona not running — ClotoCore will start it on next request")
        return True


def _verify_arm(arm_key: str) -> bool:
    """Quick smoke-test: check embedding server is reachable."""
    try:
        data = json.dumps({"texts": ["test"], "namespace": "bench"}).encode()
        req = urllib.request.Request(
            EMBED_URL, data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as r:
            r.read()
        return True
    except Exception:
        return False


def switch(arm_key: str, no_restart: bool = False):
    if arm_key not in ARMS:
        print(f"Unknown arm '{arm_key}'. Available: {', '.join(ARMS)}")
        sys.exit(1)

    arm = ARMS[arm_key]
    print(f"\nSwitching to arm: {arm_key}")
    print(f"  {arm['label']}")
    print()

    _update_db_env(arm_key)

    if no_restart:
        print("  --no-restart: skipping process restart (apply on next ClotoCore restart)")
    else:
        _restart_cpersona()

    print(f"\nArm {arm_key} ready.")
    print(f"  Next: python3 cpersona_bench_setup.py --reset && python3 cpersona_ab_runner.py --agent agent.cpersona_bench\n")


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("arm", help="Arm name (v2412 / v2413 / v2414 / list)")
    p.add_argument("--no-restart", action="store_true",
                   help="Update DB only, skip process restart")
    args = p.parse_args()

    if args.arm == "list":
        print("\nAvailable arms:")
        for k, v in ARMS.items():
            print(f"  {k:8s}  {v['label']}")
        print()
        return

    switch(args.arm, no_restart=args.no_restart)


if __name__ == "__main__":
    main()
