#!/usr/bin/env python3

import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


SCRIPT = Path(__file__).with_name("prepare_usdb_host.sh")


class PrepareUsdbHostTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.command_dir = self.root / "bin"
        self.command_dir.mkdir()
        self.os_release = self.root / "os-release"
        self.write_os_release("ubuntu", "24.04", "noble")
        for name, version in (
            ("git", "git version 2.43.0"),
            ("python3", "Python 3.12.3"),
            ("curl", "curl 8.5.0"),
            ("jq", "jq-1.7"),
        ):
            self.write_command(name, f"echo '{version}'")
        self.write_docker(compose_ok=True, daemon_ok=True)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_os_release(self, os_id: str, version: str, codename: str) -> None:
        self.os_release.write_text(
            textwrap.dedent(
                f"""\
                ID={os_id}
                VERSION_ID=\"{version}\"
                VERSION_CODENAME={codename}
                """
            ),
            encoding="utf-8",
        )

    def write_command(self, name: str, body: str) -> None:
        path = self.command_dir / name
        path.write_text(f"#!/usr/bin/env bash\nset -eu\n{body}\n", encoding="utf-8")
        path.chmod(0o755)

    def write_docker(
        self, *, compose_ok: bool, daemon_ok: bool, cgroup_version: str = "2"
    ) -> None:
        compose_status = 0 if compose_ok else 1
        daemon_status = 0 if daemon_ok else 1
        self.write_command(
            "docker",
            textwrap.dedent(
                f"""\
                case "${{1:-}}" in
                  --version)
                    echo 'Docker version 29.0.0, build test'
                    ;;
                  compose)
                    [[ "${{2:-}}" == version ]]
                    echo 'Docker Compose version v2.40.0'
                    exit {compose_status}
                    ;;
                  info)
                    echo '29.0.0|{cgroup_version}|linux'
                    exit {daemon_status}
                    ;;
                  *)
                    exit 2
                    ;;
                esac
                """
            ),
        )

    def run_script(
        self, *args: str, extra_env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "USDB_HOST_OS_RELEASE_FILE": str(self.os_release),
                "USDB_HOST_ARCH": "x86_64",
                "USDB_HOST_KERNEL_NAME": "Linux",
                "USDB_HOST_KERNEL_RELEASE": "6.8.0-test",
                "USDB_HOST_COMMAND_DIR": str(self.command_dir),
                "USDB_HOST_SKIP_SYSTEMD_CHECK": "1",
            }
        )
        env.update(extra_env or {})
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_check_accepts_supported_complete_host(self) -> None:
        result = self.run_script("check")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("INFO distribution: ubuntu 24.04", result.stdout)
        self.assertIn("PASS kernel: Linux 6.8.0-test", result.stdout)
        self.assertIn("PASS Docker Compose plugin", result.stdout)
        self.assertIn("Host prerequisite check passed.", result.stdout)

    def test_check_accepts_debian_with_supported_kernel(self) -> None:
        self.write_os_release("debian", "12", "bookworm")

        result = self.run_script("check")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("INFO distribution: debian 12", result.stdout)
        self.assertIn("automatic APT installation is supported", result.stdout)

    def test_check_rejects_missing_required_command(self) -> None:
        (self.command_dir / "jq").unlink()

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL jq: jq is missing", result.stderr)

    def test_check_rejects_missing_compose_plugin(self) -> None:
        self.write_docker(compose_ok=False, daemon_ok=True)

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL Docker Compose plugin", result.stderr)

    def test_check_rejects_inaccessible_docker_daemon(self) -> None:
        self.write_docker(compose_ok=True, daemon_ok=False)

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL Docker daemon", result.stderr)

    def test_check_rejects_unknown_docker_cgroup_mode(self) -> None:
        self.write_docker(compose_ok=True, daemon_ok=True, cgroup_version="")

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL Docker cgroup", result.stderr)

    def test_check_rejects_old_kernel(self) -> None:
        result = self.run_script(
            "check", extra_env={"USDB_HOST_KERNEL_RELEASE": "5.4.0-test"}
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected 5.10 or newer", result.stderr)

    def test_check_rejects_non_amd64_release_host(self) -> None:
        result = self.run_script("check", extra_env={"USDB_HOST_ARCH": "aarch64"})

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected x86_64", result.stderr)

    def test_install_rejects_check_only_distribution_before_mutation(self) -> None:
        self.write_os_release("alpine", "3.20", "")

        result = self.run_script("install")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("automatic install supports", result.stderr)


if __name__ == "__main__":
    unittest.main()
