"""Narrow, bundle-scoped authorization for unattended UFW status inspection.

The privileged installer is invoked with normal operator sudo authentication.
Only the distro-owned UFW status command is granted passwordless execution;
neither this helper nor a release/user-writable script is authorized by sudoers.
"""

import argparse
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys
import tempfile

SUDOERS_DIR = Path("/etc/sudoers.d")
UFW = Path("/usr/sbin/ufw")
VISUDO = Path("/usr/sbin/visudo")
INSPECTION_REQUIRED = 78


class FirewallInspectionRequired(ValueError):
    """An unattended preflight cannot inspect the actual firewall policy."""


def rule_path(bundle: str, uid: int) -> Path:
    """Use a per-network/account filename accepted by sudoers includedir."""
    if not re.fullmatch(r"usdb-(?:testnet|mainnet)-v[0-9]+", bundle) or uid < 0:
        raise ValueError("Invalid firewall permission identity")
    return SUDOERS_DIR / f"usdb-ufw-{bundle}-u{uid}"


def rule_content(bundle: str, user: str) -> str:
    """Allow exactly one command and its literal arguments, without SETENV."""
    rule_path(bundle, 0)
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.-]*\$?", user):
        raise ValueError("Invalid firewall permission user")
    return (f"# Managed by usdb-node: UFW read-only inspection for {bundle}\n"
            f"{user} ALL=(root) NOPASSWD: NOSETENV: {UFW} status verbose\n")


def _secure(path: Path) -> None:
    """Reject paths whose executable or parent directories can be replaced by users."""
    for entry in (path, *path.parents):
        info = entry.stat()
        if info.st_uid != 0 or info.st_mode & 0o022:
            raise ValueError(f"Expected a root-owned path without group/other write access: {entry}")


def installed_rule(bundle: str, uid: int) -> Path | None:
    """Inspect only our exact rule; customized or linked policy needs manual review."""
    path = rule_path(bundle, uid)
    if not SUDOERS_DIR.exists():
        return None
    _secure(SUDOERS_DIR)
    if SUDOERS_DIR.is_symlink():
        raise ValueError(f"Refusing symlinked sudoers directory: {SUDOERS_DIR}")
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError(f"Firewall permission must be a regular file without symbolic/hard links: {path}")
    _secure(path)
    expected = rule_content(bundle, pwd.getpwuid(uid).pw_name)
    with path.open() as source:
        content = source.read(4096)
    if content != expected:
        raise ValueError(f"Customized firewall permission requires manual review: {path}")
    return path


def _validate_policy(path: Path | None = None) -> None:
    """Validate with the distro's parser; never expose unrelated sudoers contents."""
    command = [str(VISUDO), "-c"] + (["-f", str(path)] if path else [])
    result = subprocess.run(command, capture_output=True, text=True, timeout=15,
                            env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"})
    if result.returncode:
        raise ValueError("Sudo policy validation failed; inspect it with sudo visudo -c")


def install_rule(bundle: str, user: str) -> None:
    """Atomically install a syntax-checked rule; leave existing custom policy untouched."""
    if os.geteuid() != 0:
        raise ValueError("Firewall permission installation requires root")
    account = pwd.getpwnam(user)
    destination = rule_path(bundle, account.pw_uid)
    content = rule_content(bundle, user)
    if account.pw_uid == 0:
        return
    for executable in (UFW, VISUDO):
        _secure(executable.resolve(strict=True))
        _secure(executable.parent)
    _secure(SUDOERS_DIR)
    existing = installed_rule(bundle, account.pw_uid)
    _validate_policy()
    if existing:
        return
    # Dotted temporary names are ignored by sudoers' includedir parser.
    descriptor, name = tempfile.mkstemp(prefix=".usdb-ufw-", dir=SUDOERS_DIR)
    temporary = Path(name)
    installed = False
    try:
        with os.fdopen(descriptor, "w") as target:
            target.write(content)
            target.flush()
            os.fchmod(target.fileno(), 0o440)
            os.fchown(target.fileno(), 0, 0)
            os.fsync(target.fileno())
        _validate_policy(temporary)
        # No replacement: a concurrent administrator's new rule must survive.
        os.link(temporary, destination)
        temporary.unlink()
        installed = True
        _validate_policy()
    except BaseException:
        if installed:
            destination.unlink()
        raise
    finally:
        temporary.unlink(missing_ok=True)


def ensure(layout, node, context) -> None:
    """Prepare permission in the interactive frontend, before starting the controller."""
    if node.configured_firewall_mode(layout) != "managed":
        return
    print("Checking background firewall access (read-only UFW status); sudo may request your password.",
          file=sys.stderr, flush=True)
    node._privileged_command(["/usr/bin/python3", "-I", str(Path(__file__).resolve()),
                              "--bundle", layout.bundle_id, "--user", context.service_user])
    # -k with a command ignores cached credentials, without revoking the login's
    # timestamp. This catches excluded sudoers.d directories and site overrides.
    command = [str(UFW), "status", "verbose"]
    if os.geteuid() != 0:
        command = ["sudo", "-k", "-n", "--", *command]
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise FirewallInspectionRequired(
            "FIREWALL_INSPECTION_REQUIRED: unattended UFW status inspection failed. "
            "Run usdb-node firewall check and review sudo policy (sudo visudo -c); "
            "then retry usdb-node up from the operator terminal. Firewall validation was not skipped.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--user", required=True)
    args = parser.parse_args()
    try:
        install_rule(args.bundle, args.user)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"Firewall inspection permission setup failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
