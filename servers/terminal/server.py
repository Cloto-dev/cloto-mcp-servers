"""
Cloto MCP Server: Terminal
Sandboxed shell command execution via MCP protocol.
Ported from plugins/terminal/src/lib.rs + sandbox.rs
"""

import asyncio
import os
import shlex
import sys
import unicodedata

sys.path.insert(0, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")))

from mcp.types import ToolAnnotations

from common.mcp_utils import ToolRegistry, run_mcp_server

# ============================================================
# Configuration (from environment variables)
# ============================================================

_DEFAULT_SANDBOX = (
    os.path.join(os.environ.get("TEMP", "C:\\Temp"), "cloto-sandbox") if os.name == "nt" else "/tmp/cloto-sandbox"
)
WORKING_DIR = os.environ.get("CLOTO_SANDBOX_DIR", _DEFAULT_SANDBOX)
MAX_OUTPUT_BYTES = int(os.environ.get("CLOTO_MAX_OUTPUT_BYTES", "65536"))
ALLOWED_COMMANDS_STR = os.environ.get("CLOTO_ALLOWED_COMMANDS", "")

ALLOWED_COMMANDS: list[str] | None = None
if ALLOWED_COMMANDS_STR:
    ALLOWED_COMMANDS = [c.strip() for c in ALLOWED_COMMANDS_STR.split(",") if c.strip()]

# ============================================================
# Sandbox: Command Validation (ported from sandbox.rs)
# ============================================================

BLOCKED_PATTERNS = [
    # ── Linux: filesystem destruction ──
    "rm -rf /",
    "rm -fr /",
    "/bin/rm -rf",
    "/usr/bin/rm -rf",
    "shred ",
    "wipefs",
    "truncate -s 0 /",
    "find / -delete",
    "find / -exec rm",
    # ── Linux: disk / partition ──
    "mkfs",
    "dd if=/dev",
    ":(){ :|:& };:",
    "> /dev/sda",
    "lvcreate",
    "lvremove",
    "vgremove",
    "pvremove",
    # ── Linux: system control ──
    # Note: "shutdown" and "reboot" are pre-existing and match as substrings.
    # These are acceptable because benign commands rarely contain these words.
    "shutdown",
    "reboot",
    "init 0",
    "init 6",
    "telinit ",
    "systemctl poweroff",
    "systemctl halt",
    "systemctl reboot",
    # ── Linux: privilege escalation ──
    "sudo ",
    "su ",
    "su\t",
    "doas ",
    "pkexec ",
    "chmod -r 777 /",
    "chmod u+s",
    "chown -r",
    # ── Linux: kernel modules ──
    "insmod ",
    "rmmod ",
    "modprobe ",
    "sysctl -w",
    # ── Linux: user / auth (command-start patterns checked separately) ──
    "useradd ",
    "userdel ",
    "usermod ",
    "chpasswd",
    # ── Linux: firewall ──
    "iptables -f",
    "iptables --flush",
    "ufw disable",
    "nft flush",
    # ── Linux: cron ──
    "crontab -r",
    # ── Code execution (anti-injection) ──
    "python -c",
    "python2 -c",
    "python3 -c",
    "perl -e",
    "ruby -e",
    "node -e",
    "php -r",
    "lua -e",
    "nc -e",
    "ncat -e",
    "socat exec:",
    # ── Pipe-to-shell (remote code execution) ──
    "| bash",
    "| sh",
    "| zsh",
    "| fish",
    "| powershell",
    "| pwsh",
    "| cmd",
    # ── Windows destructive ──
    "format-volume",
    "clear-disk",
    "remove-item -recurse -force c:",
    "remove-item -recurse -force c:\\",
    "rd /s /q c:",
    "rd /s /q c:\\",
    "del /s /q c:",
    "del /s /q c:\\",
    "reg delete hklm",
    "reg delete hkcu",
    "bcdedit /delete",
    "bcdedit /set",
    "cipher /w:c:",
    "diskpart",
    "sfc /scannow",
    "dism /online /cleanup-image",
]

# Pipe (|) is intentionally NOT blocked: it is a legitimate shell operation.
# The command approval gate in the kernel is the security boundary.
# Dangerous pipe targets are covered by BLOCKED_PATTERNS above.
BLOCKED_METACHAR_PATTERNS = [
    "$(",
    "`",
    ";",
    "&&",
    "||",
    ">",
    "<",
]

# Commands blocked by first word (avoids false positives from substring matching).
# e.g. "halt" would falsely match "asphalt"; "passwd" would match "cat /etc/passwd".
BLOCKED_FIRST_WORD = {
    "halt",
    "poweroff",
    "passwd",
    "fdisk",
    "gdisk",
    "parted",
    "cfdisk",
    "sfdisk",
}


_SANDBOX_PREFIX = "[MGP Sandbox] "
_SANDBOX_HINT = " If you need this operation, use mgp.agent.ask to delegate to an agent with appropriate permissions."


def _sandbox_error(reason: str) -> ValueError:
    return ValueError(f"{_SANDBOX_PREFIX}{reason}.{_SANDBOX_HINT}")


def validate_command(command: str) -> None:
    """Validate a command against security rules. Raises ValueError on failure.

    NOTE: The caller MUST pass an already-NFKC-normalized string so that
    the same string that is validated is also the one that gets executed.
    """
    if not command.strip():
        raise _sandbox_error("Empty command is not allowed")

    # Block embedded newlines/carriage returns and Unicode line separators
    if "\n" in command or "\r" in command or "\u2028" in command or "\u2029" in command:
        raise _sandbox_error("Command contains embedded newline or line separator (potential injection)")

    lower = command.lower()

    # Block shell metacharacters
    for meta in BLOCKED_METACHAR_PATTERNS:
        if meta in lower:
            raise _sandbox_error(f"Blocked shell metacharacter '{meta}' — use simple commands without chaining")

    # Check for blocked patterns
    for pattern in BLOCKED_PATTERNS:
        if pattern in lower:
            raise _sandbox_error(f"Blocked dangerous pattern '{pattern}'")

    # Block commands by first word (prevents false positives from substring match)
    first_word = lower.split()[0] if lower.split() else ""
    if first_word in BLOCKED_FIRST_WORD:
        raise _sandbox_error(f"Command '{first_word}' is a restricted system operation")

    # Block rm with both -r and -f flags
    normalized = " ".join(lower.split())
    if normalized.startswith("rm ") or "/rm " in normalized:
        tokens = normalized.split()
        has_recursive = any(t.startswith("-") and not t.startswith("--") and ("r" in t or "R" in t) for t in tokens)
        has_force = any(t.startswith("-") and not t.startswith("--") and "f" in t for t in tokens)
        if has_recursive and has_force:
            raise _sandbox_error("Dangerous rm flags (-r and -f combined) are not allowed")

    # If an allowlist is configured, check the first word
    if ALLOWED_COMMANDS is not None:
        first_word = command.split()[0] if command.split() else ""
        if first_word not in ALLOWED_COMMANDS:
            raise _sandbox_error(
                f"Command '{first_word}' is not in the allowlist (allowed: {', '.join(ALLOWED_COMMANDS)})"
            )


def safe_truncate(s: str, max_bytes: int) -> str:
    """Safely truncate a string at a UTF-8 byte boundary."""
    encoded = s.encode("utf-8")
    if len(encoded) <= max_bytes:
        return s
    truncated = encoded[:max_bytes]
    return truncated.decode("utf-8", errors="ignore")


# ============================================================
# MCP Server
# ============================================================

registry = ToolRegistry("cloto-mcp-terminal")


@registry.tool(
    "execute_command",
    "Execute a shell command and return stdout, stderr, and exit code. "
    "Use this to run scripts, check file contents, inspect system state, "
    "compile code, run tests, or perform any command-line operation.",
    {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell command to execute",
            },
            "timeout_secs": {
                "type": "integer",
                "description": "Timeout in seconds (default: 30, max: 120)",
            },
        },
        "required": ["command"],
    },
    annotations=ToolAnnotations(destructiveHint=True),
)
async def handle_execute(arguments: dict) -> dict:
    command = arguments.get("command")
    if not command:
        return {"exit_code": -1, "stdout": "", "stderr": "Missing 'command' argument"}

    # NFKC normalization BEFORE validation so the same string is validated and executed
    command = unicodedata.normalize("NFKC", command)

    timeout_secs = min(arguments.get("timeout_secs", 30), 120)

    # Validate command against sandbox rules
    try:
        validate_command(command)
    except ValueError as e:
        return {"exit_code": -1, "stdout": "", "stderr": str(e)}

    # Ensure working directory exists
    os.makedirs(WORKING_DIR, exist_ok=True)

    try:
        try:
            argv = shlex.split(command)
        except ValueError as e:
            return {"exit_code": -1, "stdout": "", "stderr": f"Failed to parse command: {e}"}
        proc = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=WORKING_DIR,
        )

        try:
            stdout_bytes, stderr_bytes = await asyncio.wait_for(proc.communicate(), timeout=timeout_secs)
        except asyncio.TimeoutError:
            proc.kill()
            await proc.wait()
            return {"exit_code": -1, "stdout": "", "stderr": f"Command timed out after {timeout_secs} seconds"}

        stdout = stdout_bytes.decode("utf-8", errors="replace")
        stderr = stderr_bytes.decode("utf-8", errors="replace")

        # Safe UTF-8 truncation
        if len(stdout.encode("utf-8")) > MAX_OUTPUT_BYTES:
            stdout = safe_truncate(stdout, MAX_OUTPUT_BYTES) + f"...[truncated, {len(stdout_bytes)} bytes total]"
        if len(stderr.encode("utf-8")) > MAX_OUTPUT_BYTES:
            stderr = safe_truncate(stderr, MAX_OUTPUT_BYTES) + f"...[truncated, {len(stderr_bytes)} bytes total]"

        exit_code = proc.returncode if proc.returncode is not None else -1

        return {"exit_code": exit_code, "stdout": stdout, "stderr": stderr}

    except Exception as e:
        return {"exit_code": -1, "stdout": "", "stderr": f"Failed to execute command: {e}"}


if __name__ == "__main__":
    asyncio.run(run_mcp_server(registry))
