"""Collect the existing preflight checks into an operator-facing terminal report."""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from dataclasses import dataclass, field
import io
import os
import re
import shutil
import subprocess
import textwrap
from typing import Any, Callable


# The order matches doctor(): later checks remain explicitly unobserved on failure.
SECTIONS = {
    "release": ("Release", "Release manifest and bundled network validated"),
    "configuration": ("Configuration and resources", "Private configuration loaded; memory budget validated"),
    "host": ("Host and Docker", "Host prerequisites checked"),
    "data": ("Data and bootstrap", "Data identity, credentials and bootstrap configuration checked"),
    "images": ("Container images", "Image references match this release"),
    "network": ("P2P network", "Configured address family meets local host requirements"),
    "runtime": ("Runtime configuration", "Bundle and node settings validated"),
    "registry": ("Script registry", "Registry state inspected"),
    "firewall": ("Firewall", "Managed firewall policy checked"),
}
CHECK_ERRORS = (OSError, ValueError, subprocess.SubprocessError)
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*(?:\x07|\x1b\\)")
STATES = {"WARNING": "WARN", "ERROR": "FAIL", "SKIPPED": "SKIP", "OK": "PASS"}
COLORS = {"PASS": "32", "FAIL": "31", "WAIT": "33", "WARN": "33", "PENDING": "36",
          "INFO": "90", "SKIP": "90"}


@dataclass
class Finding:
    state: str
    label: str
    message: str


@dataclass
class Section:
    state: str = "NOT CHECKED"
    findings: list[Finding] = field(default_factory=list)


class DoctorReport:
    """Capture check evidence without changing its outcome or running recovery actions."""

    def __init__(self) -> None:
        self.sections = {key: Section() for key in SECTIONS}
        self.release_id = "release unavailable"
        self.identity = ""
        self.failed = False
        self.secrets: list[str] = []

    def identify(self, layout: Any, env: dict[str, str] | None = None) -> None:
        self.release_id = layout.release_id
        network = layout.network_identity
        chain_id = network.get("chain_id", "unknown")
        self.identity = f"Network: {layout.bundle_id} | Chain ID: {chain_id}"
        if env is not None:
            self.identity += f" | Role: {env.get('USDB_NODE_ROLE', 'unknown')}"
            self.secrets = [value for key, value in env.items() if value and
                            any(word in key for word in ("PASSWORD", "TOKEN", "SECRET", "RPCAUTH"))]

    def add(self, section: str, state: str, label: str, message: str) -> None:
        finding = Finding(STATES.get(state, state), label, message)
        if finding not in self.sections[section].findings:
            self.sections[section].findings.append(finding)

    def _output(self, section: str, output: str) -> None:
        """Keep helper diagnostics, grouping their stable PASS/FAIL prefixes as rows."""
        for line in output.splitlines():
            line = line.strip()
            if not line:
                continue
            match = re.match(r"^(PASS|FAIL|WARN|WARNING|WAIT|INFO|PENDING|ERROR)\s+([^:]+):\s*(.*)$", line)
            if match:
                self.add(section, match[1], match[2], match[3])
            elif (match := re.match(r"^(ERROR|FAIL|WARN|WARNING):\s*(.*)$", line)):
                self.add(section, match[1], SECTIONS[section][0], match[2])
            elif line.startswith("network bundle validation failed:"):
                self.add(section, "FAIL", "Network bundle", line.split(":", 1)[1].strip())
            elif line.startswith("ACTION REQUIRED [DOCKER_SESSION_REFRESH_REQUIRED]"):
                # The concise recovery step appears in Needs attention below.
                continue
            elif line.startswith(("Host prerequisite check ", "Host preparation paused:")):
                continue
            elif section == "host" and self.sections[section].findings and line.startswith((
                "Account ", "This is common ", "Recommended:", "Alternative:", "In the new ",
                "Not configured yet:", "Already configured:", "'exit' ", "For this session ",
                "Other checks also failed;",
            )):
                continue
            else:
                self.add(section, "INFO", "detail", line)

    def check(self, section: str, action: Callable[[], Any]) -> Any:
        """Record both successful and failed evidence, then preserve the original exception."""
        output = io.StringIO()
        current = self.sections[section]
        try:
            with redirect_stdout(output), redirect_stderr(output):
                result = action()
            if isinstance(result, subprocess.CompletedProcess):
                output.write(result.stdout or "")
                output.write(result.stderr or "")
                result.check_returncode()
        except CHECK_ERRORS as error:
            # Host-action wrappers keep their captured helper evidence in the exception context.
            helper_error = error if isinstance(error, subprocess.CalledProcessError) else error.__context__
            if isinstance(helper_error, subprocess.CalledProcessError):
                output.write(helper_error.stdout or "")
                output.write(helper_error.stderr or "")
            if isinstance(error, subprocess.CalledProcessError):
                message = f"Check command failed (exit {error.returncode}); see diagnostics below."
            else:
                message = str(error)
            self._output(section, output.getvalue())
            current.state = "WAIT" if message.startswith("DOCKER_SESSION_REFRESH_REQUIRED:") else "FAIL"
            self.add(section, current.state, SECTIONS[section][0], message)
            self.failed = True
            raise
        else:
            self._output(section, output.getvalue())
            current.state = "PASS"
            return result

    def render(self, *, width: int = 100, color: bool = False, unicode: bool = False) -> str:
        """Render a summary followed by aligned, wrapped evidence; never truncate errors."""
        width = max(40, width)
        lines: list[str] = []
        symbols = {"PASS": "✓", "FAIL": "✗", "WAIT": "!", "WARN": "!", "INFO": "·",
                   "PENDING": "→", "SKIP": "-"} if unicode else {}

        def clean(value: str) -> str:
            for secret in sorted(self.secrets, key=len, reverse=True):
                value = value.replace(secret, "[redacted]")
            value = ANSI.sub("", value)
            return " ".join("".join(c for c in value if c.isprintable() or c.isspace()).split())

        def text(value: str, prefix: str = "", indent: str | None = None, state: str = "") -> None:
            wrapped = textwrap.wrap(clean(value), width=width, initial_indent=prefix,
                                    subsequent_indent=prefix if indent is None else indent,
                                    break_long_words=True, break_on_hyphens=False) or [prefix.rstrip()]
            for line in wrapped:
                lines.append(f"\033[{COLORS[state]}m{line}\033[0m" if color and state in COLORS else line)

        def finding(item: Finding) -> None:
            status = f"{symbols.get(item.state, item.state)} {item.state}" if unicode else item.state
            label = clean(item.label)
            if width >= 80 and len(label) <= 23:
                text(item.message, f"  {status:<10} {label:<24} ", " " * 38, item.state)
            else:
                text(f"{label}: {item.message}", f"  {status:<10} ", " " * 13, item.state)

        def heading(value: str) -> None:
            lines.append("")
            text(value)
            if color:
                lines[-1] = f"\033[1m{lines[-1]}\033[0m"

        text(f"USDB Doctor | {self.release_id}")
        if self.identity:
            text(self.identity)
        attention = [item for section in self.sections.values() for item in section.findings
                     if item.state in {"FAIL", "WAIT", "WARN"}]
        unchecked = [SECTIONS[key][0] for key, section in self.sections.items() if section.state == "NOT CHECKED"]
        result = "ACTION REQUIRED" if self.failed else "PASSED WITH NOTES" if attention else "PASSED"
        text(f"Result: {result}" + (f" | {len(unchecked)} groups not checked" if unchecked else ""))
        heading("Needs attention")
        if attention:
            # Show concrete failures first; generic helper summaries stay in their group.
            for key, section in self.sections.items():
                issues = [item for item in section.findings if item.state in {"FAIL", "WAIT", "WARN"}]
                concrete = [item for item in issues if not item.message.startswith((
                    "HOST_PREREQUISITES_FAILED:", "DOCKER_SESSION_REFRESH_REQUIRED:", "Check command failed (",
                ))]
                for item in concrete or issues:
                    finding(item)
        else:
            text("No blocking issues found in the completed preflight checks.", "  ")

        for key, section in self.sections.items():
            if section.state == "NOT CHECKED":
                continue
            title, summary = SECTIONS[key]
            heading(title)
            if section.state == "PASS" and not any(item.state in {"PASS", "SKIP", "WARN", "PENDING"}
                                                    for item in section.findings):
                finding(Finding("PASS", "check", summary))
            for item in section.findings:
                finding(item)
        if unchecked:
            heading("Not checked")
            text("Stopped at the first blocking check; these results are unknown:", "  ")
            for title in unchecked:
                text(title, "  - ")
        heading("Next steps")
        if self.failed:
            if any("DOCKER_SESSION_REFRESH_REQUIRED" in item.message for item in attention):
                text("Reconnect using the same SSH command and operator account, or run newgrp docker.", "  1. ", "     ")
                text("In the new shell: usdb-node host check && usdb-node doctor", "  2. ", "     ")
                text("No reinstall, repeated setup or reboot is needed for this session change; fix any other FAIL items too.", "     ")
            elif self.sections["configuration"].state == "FAIL" and any("not configured" in item.message for item in attention):
                text("Run usdb-node setup, then usdb-node doctor.", "  ")
            else:
                text("Resolve the reported issue, then rerun usdb-node doctor.", "  ")
        else:
            text("Preflight passed; this does not mean services are running or synchronized.", "  ")
            text("Start when ready: usdb-node up", "  ")
            text("For an already running node: usdb-node status --watch", "  ")
        return "\n".join(lines) + "\n"

    def print(self, output: Any) -> None:
        """Use decoration only for supported interactive terminals, never in log files."""
        tty = output.isatty()
        terminal = tty and os.environ.get("TERM", "") != "dumb"
        encoding = getattr(output, "encoding", "") or ""
        width = shutil.get_terminal_size((100, 24)).columns if tty else 100
        output.write(self.render(width=width, color=terminal and "NO_COLOR" not in os.environ,
                                 unicode=terminal and encoding.lower().replace("-", "") == "utf8"))
