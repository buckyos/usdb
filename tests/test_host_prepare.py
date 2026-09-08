#!/usr/bin/env python3
"""Exercise host installation with isolated package, network and service commands."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "docker/scripts/tools/prepare_usdb_host.sh"


class HostPrepareInstallTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        # Only harmless host utilities are reachable; privileged commands are stubs.
        for name in ("bash", "cat", "grep", "mktemp", "rm"):
            (self.bin / name).symlink_to(shutil.which(name))
        commands = {
            "sudo": 'exec "$@"',
            "apt-get": '''
                echo "$*" >> "$USDB_TEST_ROOT/apt-calls"
                if [[ " $* " == *" docker-ce "* ]]; then
                  : > "$USDB_TEST_ROOT/docker-installed"
                fi
            ''',
            "dpkg": '[[ "$*" == --print-architecture ]]; echo amd64',
            "dpkg-query": '''
                [[ "${USDB_TEST_CONFLICT:-0}" == 1 && "${!#}" == docker.io ]] || exit 1
                echo 'ii '
            ''',
            "curl": '''
                if [[ "$*" == --version ]]; then
                  echo 'curl 8.18.0'
                else
                  [[ "$1" == -fsSL && "$3" == -o ]]
                  echo "$2" > "$USDB_TEST_ROOT/key-url"
                  echo 'fixture key' > "$4"
                fi
            ''',
            "install": '''
                case "$*" in
                  '-m 0755 -d /etc/apt/keyrings') ;;
                  *)
                    [[ "$1" == -m && "$2" == 0644 ]]
                    case "$4" in
                      /etc/apt/keyrings/docker.asc|/etc/apt/sources.list.d/docker.sources)
                        cat "$3" > "$USDB_TEST_ROOT/${4##*/}" ;;
                      *) exit 2 ;;
                    esac
                    ;;
                esac
            ''',
            "systemctl": 'echo "$*" >> "$USDB_TEST_ROOT/systemctl-calls"',
            "docker": '''
                [[ -f "$USDB_TEST_ROOT/docker-installed" ]] || exit 1
                case "$*" in
                  --version) echo 'Docker version 29.0.0' ;;
                  'compose version') echo 'Docker Compose version v2.40.0' ;;
                  'info --format '*) echo '29.0.0|2|linux' ;;
                  *) exit 2 ;;
                esac
            ''',
            "git": "echo 'git version 2.43.0'",
            "python3": "echo 'Python 3.14.4'",
            "jq": "echo 'jq-1.7'",
        }
        for name, body in commands.items():
            path = self.bin / name
            path.write_text("#!/usr/bin/env bash\nset -eu\n" + textwrap.dedent(body))
            path.chmod(0o755)

    def run_install(
        self, os_id: str, version: str, codename: str, *, conflict: bool = False
    ) -> subprocess.CompletedProcess[str]:
        os_release = self.root / "os-release"
        os_release.write_text(
            f'ID={os_id}\nVERSION_ID="{version}"\nVERSION_CODENAME={codename}\n'
        )
        return subprocess.run(
            [str(self.bin / "bash"), str(SCRIPT), "install"],
            env={
                **os.environ,
                "PATH": str(self.bin),
                "TMPDIR": str(self.root),
                "USDB_TEST_ROOT": str(self.root),
                "USDB_TEST_CONFLICT": "1" if conflict else "0",
                "USDB_HOST_OS_RELEASE_FILE": str(os_release),
                "USDB_HOST_ARCH": "x86_64",
                "USDB_HOST_KERNEL_NAME": "Linux",
                "USDB_HOST_KERNEL_RELEASE": "7.0.0-30-generic",
                "USDB_HOST_COMMAND_DIR": str(self.bin),
                "USDB_HOST_SKIP_SYSTEMD_CHECK": "1",
            },
            text=True,
            capture_output=True,
            timeout=10,
        )

    def test_install_uses_native_docker_repository_for_each_supported_release(self) -> None:
        for os_id, version, codename in (
            ("ubuntu", "22.04", "jammy"),
            ("ubuntu", "24.04", "noble"),
            ("ubuntu", "26.04", "resolute"),
            ("debian", "12", "bookworm"),
            ("debian", "13", "trixie"),
        ):
            with self.subTest(os_id=os_id, version=version):
                (self.root / "docker-installed").unlink(missing_ok=True)
                (self.root / "apt-calls").unlink(missing_ok=True)
                result = self.run_install(os_id, version, codename)

                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("Host prerequisite check passed.", result.stdout)
                self.assertEqual(
                    (self.root / "docker.sources").read_text(),
                    "Types: deb\n"
                    f"URIs: https://download.docker.com/linux/{os_id}\n"
                    f"Suites: {codename}\n"
                    "Components: stable\nArchitectures: amd64\n"
                    "Signed-By: /etc/apt/keyrings/docker.asc\n",
                )
                self.assertEqual(
                    (self.root / "key-url").read_text(),
                    f"https://download.docker.com/linux/{os_id}/gpg\n",
                )
                self.assertEqual((self.root / "docker.asc").read_text(), "fixture key\n")
                self.assertEqual(
                    (self.root / "apt-calls").read_text().splitlines(),
                    [
                        "update",
                        "install -y ca-certificates curl git python3 jq",
                        "update",
                        "install -y docker-ce docker-ce-cli containerd.io "
                        "docker-buildx-plugin docker-compose-plugin",
                    ],
                )

    def test_install_rejects_unlisted_ubuntu_before_package_changes(self) -> None:
        result = self.run_install("ubuntu", "28.04", "unknown")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("automatic install supports Ubuntu 22.04/24.04/26.04", result.stderr)
        self.assertFalse((self.root / "apt-calls").exists())
        self.assertFalse((self.root / "docker.sources").exists())

    def test_ubuntu_26_04_conflicting_packages_block_installation(self) -> None:
        result = self.run_install("ubuntu", "26.04", "resolute", conflict=True)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Conflicting container packages are installed: docker.io", result.stderr)
        self.assertFalse((self.root / "apt-calls").exists())
        self.assertFalse((self.root / "docker.sources").exists())


if __name__ == "__main__":
    unittest.main()
