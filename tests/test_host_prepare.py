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
KEY_FIXTURE = Path(__file__).with_name("common") / "docker-signing-key.asc"


class HostPrepareInstallTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.apt_sources = self.root / "apt-sources"
        self.apt_sources.mkdir()
        (self.apt_sources / "ubuntu.sources").write_text("Existing OS repository\n")
        (self.apt_sources / "operator.list").write_text("Existing operator repository\n")
        # Simulate a failed prior attempt, including no usable cached Docker index.
        (self.apt_sources / "docker.sources").write_text("Unreachable old Docker repository\n")
        # Only harmless host utilities are reachable; privileged commands are stubs.
        for name in ("bash", "cat", "grep", "mktemp", "rm", "rmdir", "chmod", "ln", "sha256sum"):
            (self.bin / name).symlink_to(shutil.which(name))
        commands = {
            "sudo": 'exec "$@"',
            "apt-get": '''
                [[ " $* " == *" -o Acquire::Retries=2 "* ]]
                [[ " $* " == *" -o Acquire::http::Timeout=30 "* ]]
                [[ " $* " == *" -o Acquire::https::Timeout=30 "* ]]
                bootstrap=0
                while [[ "${1:-}" == -o ]]; do
                  if [[ "$2" == Dir::Etc::sourceparts=* ]]; then
                    parts="${2#*=}"
                    [[ -f "$parts/ubuntu.sources" && -f "$parts/operator.list" ]]
                    [[ ! -e "$parts/docker.sources" ]]
                    bootstrap=1
                  fi
                  shift 2
                done
                echo "$*" >> "$USDB_TEST_ROOT/apt-calls"
                if [[ " $* " == *" docker-ce "* ]]; then
                  if [[ " $* " == *" --download-only "* ]]; then
                    if [[ "$USDB_TEST_FAILURE" == download ]] && source_fails; then exit 100; fi
                    : > "$USDB_TEST_ROOT/cached-packages"
                  else
                    [[ " $* " == *" --no-download "* ]]
                    [[ -f "$USDB_TEST_ROOT/cached-packages" ]]
                    if [[ "$USDB_TEST_FAILURE" == install ]]; then exit 100; fi
                    : > "$USDB_TEST_ROOT/docker-installed"
                  fi
                elif [[ "$*" == 'update --error-on=any' ]]; then
                  if [[ "$bootstrap" == 1 ]]; then
                    if [[ "$USDB_TEST_FAILURE" == bootstrap ]]; then exit 100; fi
                  elif [[ "$USDB_TEST_FAILURE" == update ]] && source_fails; then
                    exit 100
                  fi
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
                  echo "$*" >> "$USDB_TEST_ROOT/curl-calls"
                  [[ " $* " == *" --retry 2 "* && " $* " == *" --retry-all-errors "* ]]
                  [[ " $* " == *" --retry-delay 2 "* && " $* " == *" --retry-max-time 90 "* ]]
                  [[ " $* " == *" --connect-timeout 10 "* && " $* " == *" --max-time 30 "* ]]
                  [[ " $* " == *" --proto =https "* && " $* " == *" --proto-redir =https "* ]]
                  while (($# > 3)); do shift; done
                  [[ "$2" == -o ]]
                  echo "$1" > "$USDB_TEST_ROOT/key-url"
                  if [[ "$USDB_TEST_FAILURE" == key ]] && source_fails "$1"; then exit 35; fi
                  if [[ "$USDB_TEST_FAILURE" == key-mismatch ]]; then
                    echo 'unexpected signing key' > "$3"
                  else
                    cat "$USDB_TEST_KEY_FIXTURE" > "$3"
                  fi
                fi
            ''',
            "install": '''
                case "$*" in
                  '-m 0755 -d /etc/apt/keyrings') ;;
                  *)
                    [[ "$1" == -m && "$2" == 0644 ]]
                    case "$4" in
                      /etc/apt/keyrings/docker.asc|"$USDB_HOST_APT_SOURCES_DIR/docker.sources")
                        if [[ "$4" == */docker.sources && "$USDB_TEST_FAILURE" == write-source ]]; then exit 77; fi
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
            path.write_text("#!/usr/bin/env bash\nset -eu\n" + textwrap.dedent('''
                source_fails() {
                  [[ "$USDB_TEST_FAIL_SOURCE" == all ]] && return 0
                  local source="${1:-}"
                  if [[ -z "$source" && -f "$USDB_TEST_ROOT/docker.sources" ]]; then
                    source="$(cat "$USDB_TEST_ROOT/docker.sources")"
                  fi
                  [[ "$source" == *download.docker.com* ]]
                }
            ''') + textwrap.dedent(body))
            path.chmod(0o755)

    def run_install(
        self, os_id: str = "ubuntu", version: str = "26.04", codename: str = "resolute",
        *, conflict: bool = False, mirror: str = "auto", failure: str = "",
        fail_source: str = "official", existing_docker: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        os_release = self.root / "os-release"
        os_release.write_text(
            f'ID={os_id}\nVERSION_ID="{version}"\nVERSION_CODENAME={codename}\n'
        )
        for name in ("docker-installed", "cached-packages", "apt-calls", "curl-calls",
                     "key-url", "docker.sources", "docker.asc", "systemctl-calls"):
            (self.root / name).unlink(missing_ok=True)
        if existing_docker:
            (self.root / "docker-installed").touch()
        return subprocess.run(
            [str(self.bin / "bash"), str(SCRIPT), "install", "--docker-mirror", mirror],
            env={
                **os.environ,
                "PATH": str(self.bin),
                "TMPDIR": str(self.root),
                "USDB_TEST_ROOT": str(self.root),
                "USDB_TEST_CONFLICT": "1" if conflict else "0",
                "USDB_TEST_FAILURE": failure,
                "USDB_TEST_FAIL_SOURCE": fail_source,
                "USDB_TEST_KEY_FIXTURE": str(KEY_FIXTURE),
                "USDB_HOST_OS_RELEASE_FILE": str(os_release),
                "USDB_HOST_APT_SOURCES_DIR": str(self.apt_sources),
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
                self.assertEqual((self.root / "docker.asc").read_bytes(), KEY_FIXTURE.read_bytes())
                self.assertEqual(
                    (self.root / "apt-calls").read_text().splitlines(),
                    [
                        "update --error-on=any",
                        "install -y ca-certificates curl git python3 jq",
                        "update --error-on=any",
                        "install -y --download-only docker-ce docker-ce-cli containerd.io "
                        "docker-buildx-plugin docker-compose-plugin",
                        "install -y --no-download docker-ce docker-ce-cli containerd.io "
                        "docker-buildx-plugin docker-compose-plugin",
                    ],
                )

    def test_auto_falls_back_after_key_index_or_package_download_failure(self) -> None:
        for stage in ("key", "update", "download"):
            with self.subTest(stage=stage):
                result = self.run_install(failure=stage)

                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("falling back to Tsinghua", result.stderr)
                self.assertIn("mirrors.tuna.tsinghua.edu.cn/docker-ce/linux/ubuntu",
                              (self.root / "docker.sources").read_text())
                self.assertIn("mirrors.tuna.tsinghua.edu.cn/docker-ce/linux/ubuntu/gpg",
                              (self.root / "key-url").read_text())
                self.assertEqual((self.root / "apt-calls").read_text().count("--no-download"), 1)

    def test_explicit_tuna_never_contacts_official_source(self) -> None:
        result = self.run_install(mirror="tuna", failure="key")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = (self.root / "curl-calls").read_text()
        self.assertNotIn("download.docker.com", calls)
        self.assertIn("mirrors.tuna.tsinghua.edu.cn", calls)
        self.assertNotIn("falling back", result.stderr)

    def test_explicit_official_does_not_fall_back(self) -> None:
        result = self.run_install(mirror="official", failure="key")

        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("mirrors.tuna.tsinghua.edu.cn", (self.root / "curl-calls").read_text())
        self.assertNotIn("falling back", result.stderr)
        self.assertFalse((self.root / "docker-installed").exists())

    def test_both_sources_failing_never_runs_package_installation(self) -> None:
        for stage in ("key", "update", "download"):
            with self.subTest(stage=stage):
                result = self.run_install(failure=stage, fail_source="all")

                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Docker repository acquisition failed", result.stderr)
                self.assertNotIn("--no-download", (self.root / "apt-calls").read_text())
                self.assertFalse((self.root / "docker-installed").exists())
                self.assertFalse((self.root / "systemctl-calls").exists())

    def test_untrusted_key_is_rejected_before_installing_it(self) -> None:
        result = self.run_install(mirror="tuna", failure="key-mismatch")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Docker signing key checksum mismatch", result.stderr)
        self.assertFalse((self.root / "docker.asc").exists())
        self.assertFalse((self.root / "docker.sources").exists())

    def test_local_write_or_dpkg_failure_does_not_trigger_fallback(self) -> None:
        for stage in ("write-source", "install"):
            with self.subTest(stage=stage):
                result = self.run_install(failure=stage)

                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("falling back", result.stderr)
                self.assertNotIn("mirrors.tuna.tsinghua.edu.cn", (self.root / "curl-calls").read_text())
                self.assertFalse((self.root / "docker-installed").exists())

    def test_complete_existing_docker_installation_is_preserved(self) -> None:
        result = self.run_install(existing_docker=True, failure="key", fail_source="all")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("preserving the existing installation", result.stdout)
        self.assertFalse((self.root / "curl-calls").exists())
        self.assertFalse((self.root / "docker.sources").exists())

    def test_invalid_mirror_is_rejected_before_mutation(self) -> None:
        result = self.run_install(mirror="unknown")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid Docker mirror", result.stderr)
        self.assertFalse((self.root / "apt-calls").exists())

    def test_failed_base_repository_does_not_attempt_docker_install(self) -> None:
        result = self.run_install(failure="bootstrap")

        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "curl-calls").exists())
        self.assertEqual((self.root / "apt-calls").read_text(), "update --error-on=any\n")
        self.assertEqual((self.apt_sources / "docker.sources").read_text(),
                         "Unreachable old Docker repository\n")

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
