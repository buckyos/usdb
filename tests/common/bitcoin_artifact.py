"""Generate isolated upstream signing fixtures with real GPG signatures."""

import hashlib
from pathlib import Path
import subprocess


class UpstreamFixture:
    def __init__(self, root: Path):
        self.root = root
        self.home = root / "gpg"
        self.home.mkdir(mode=0o700)
        self.keys = {}
        self.fingerprints = []
        for index in range(3):
            identity = f"USDB Test Signer {index} <test{index}@example.invalid>"
            self.gpg("--pinentry-mode", "loopback", "--passphrase", "", "--quick-generate-key", identity, "ed25519", "sign", "0")
            entries = self.gpg("--with-colons", "--list-keys", identity).decode().splitlines()
            fingerprint = next(line.split(":")[9] for line in entries if line.startswith("fpr:"))
            self.fingerprints.append(fingerprint)
            name = f"test{index}.gpg"
            key = self.gpg("--export", fingerprint)
            (root / name).write_bytes(key)
            self.keys[name] = (hashlib.sha256(key).hexdigest(), fingerprint)

    def gpg(self, *arguments):
        return subprocess.run(["gpg", "--batch", "--no-options", "--homedir", str(self.home), *arguments],
                              check=True, capture_output=True, timeout=30).stdout

    def sign_sums(self, count=3):
        signers = [arg for fingerprint in self.fingerprints[:count] for arg in ("--local-user", fingerprint)]
        signature = self.gpg("--yes", "--armor", *signers, "--detach-sign", "--output", "-", str(self.root / "SHA256SUMS"))
        (self.root / "SHA256SUMS.asc").write_bytes(signature)

    def close(self):
        subprocess.run(["gpgconf", "--homedir", str(self.home), "--kill", "gpg-agent"], capture_output=True, timeout=15)
