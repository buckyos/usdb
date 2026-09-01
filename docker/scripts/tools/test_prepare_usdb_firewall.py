#!/usr/bin/env python3

import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


SCRIPT = Path(__file__).with_name("prepare_usdb_firewall.sh")


class PrepareUsdbFirewallTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.command_dir = self.root / "bin"
        self.command_dir.mkdir()
        self.node_env = self.root / "node.env"
        self.status_file = self.root / "ufw-status"
        self.call_log = self.root / "ufw-calls"
        self.write_node_env(bitcoin_bind="127.0.0.1")
        self.write_status(bitcoin_public=False)
        self.write_ufw()

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_node_env(
        self,
        *,
        bitcoin_bind: str,
        http_bind: str = "127.0.0.1",
    ) -> None:
        self.node_env.write_text(
            textwrap.dedent(
                f"""\
                BTC_P2P_BIND_ADDRESS={bitcoin_bind}
                BTC_P2P_BIND_PORT=8333
                USDB_P2P_BIND_ADDRESS=0.0.0.0
                USDB_P2P_BIND_PORT=31303
                USDB_HTTP_BIND_ADDRESS={http_bind}
                USDB_WS_BIND_ADDRESS=127.0.0.1
                BH_BIND_ADDRESS=127.0.0.1
                USDB_INDEXER_BIND_ADDRESS=127.0.0.1
                CONTROL_PLANE_BIND_ADDRESS=127.0.0.1
                """
            ),
            encoding="utf-8",
        )

    def write_status(
        self,
        *,
        bitcoin_public: bool,
        active: bool = True,
        include_sensitive: bool = False,
    ) -> None:
        lines = [
            f"Status: {'active' if active else 'inactive'}",
            "Logging: on (low)",
            "Default: deny (incoming), allow (outgoing), disabled (routed)",
            "",
            "To                         Action      From",
            "--                         ------      ----",
            "22/tcp                     ALLOW IN    Anywhere",
            "31303/tcp                  ALLOW IN    Anywhere",
            "31303/udp                  ALLOW IN    Anywhere",
        ]
        if bitcoin_public:
            lines.append("8333/tcp                   ALLOW IN    Anywhere")
        if include_sensitive:
            lines.append("8545/tcp                   ALLOW IN    Anywhere")
        self.status_file.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def write_ufw(self, command_dir: Path | None = None) -> None:
        path = (command_dir or self.command_dir) / "ufw"
        path.write_text(
            textwrap.dedent(
                """\
                #!/usr/bin/env bash
                set -eu
                if [[ "$*" == "status verbose" ]]; then
                  cat "${USDB_TEST_UFW_STATUS_FILE}"
                  exit 0
                fi
                printf '%s\n' "$*" >>"${USDB_TEST_UFW_CALL_LOG}"
                if [[ "$*" == "--force delete allow 8333/tcp" ]]; then
                  grep -v '^8333/tcp' "${USDB_TEST_UFW_STATUS_FILE}" >"${USDB_TEST_UFW_STATUS_FILE}.tmp"
                  mv "${USDB_TEST_UFW_STATUS_FILE}.tmp" "${USDB_TEST_UFW_STATUS_FILE}"
                fi
                """
            ),
            encoding="utf-8",
        )
        path.chmod(0o755)

    def run_script(
        self,
        action: str,
        *,
        bitcoin_p2p: str = "private",
        confirm: bool = False,
        use_command_dir: bool = True,
        system_command_dirs: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
        args = [
            "bash",
            str(SCRIPT),
            action,
            "--node-env",
            str(self.node_env),
            "--ssh-port",
            "22",
            "--bitcoin-p2p",
            bitcoin_p2p,
        ]
        if confirm:
            args.append("--confirm")
        env = os.environ.copy()
        env.update(
            {
                "USDB_FIREWALL_SKIP_ROOT": "1",
                "USDB_TEST_UFW_STATUS_FILE": str(self.status_file),
                "USDB_TEST_UFW_CALL_LOG": str(self.call_log),
            }
        )
        if use_command_dir:
            env["USDB_FIREWALL_COMMAND_DIR"] = str(self.command_dir)
        else:
            env.pop("USDB_FIREWALL_COMMAND_DIR", None)
        if system_command_dirs is not None:
            env["USDB_FIREWALL_SYSTEM_COMMAND_DIRS"] = str(system_command_dirs)
            env["PATH"] = "/usr/bin:/bin"
        return subprocess.run(
            args,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_check_accepts_private_bitcoin_profile(self) -> None:
        result = self.run_script("check")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("PASS Bitcoin P2P binding: loopback-only", result.stdout)
        self.assertIn("Firewall check passed", result.stdout)

    def test_check_resolves_ufw_when_login_path_omits_sbin(self) -> None:
        system_sbin = self.root / "system-sbin"
        system_sbin.mkdir()
        self.write_ufw(system_sbin)

        result = self.run_script(
            "check",
            use_command_dir=False,
            system_command_dirs=system_sbin,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Firewall check passed", result.stdout)

    def test_check_accepts_public_bitcoin_profile(self) -> None:
        self.write_node_env(bitcoin_bind="0.0.0.0")
        self.write_status(bitcoin_public=True)

        result = self.run_script("check", bitcoin_p2p="public")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("PASS UFW allow: 8333/tcp", result.stdout)

    def test_check_rejects_private_mode_with_public_bitcoin_bind(self) -> None:
        self.write_node_env(bitcoin_bind="0.0.0.0")

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BTC_P2P_BIND_ADDRESS must be 127.0.0.1", result.stderr)

    def test_check_rejects_public_operator_rpc(self) -> None:
        self.write_node_env(bitcoin_bind="127.0.0.1", http_bind="0.0.0.0")

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("USDB_HTTP_BIND_ADDRESS must be 127.0.0.1", result.stderr)

    def test_check_rejects_sensitive_ufw_allow(self) -> None:
        self.write_status(bitcoin_public=False, include_sensitive=True)

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sensitive port 8545/tcp is explicitly allowed", result.stderr)

    def test_check_rejects_inactive_ufw(self) -> None:
        self.write_status(bitcoin_public=False, active=False)

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected active", result.stderr)

    def test_check_rejects_bitcoin_allow_in_private_mode(self) -> None:
        self.write_status(bitcoin_public=True)

        result = self.run_script("check")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("8333/tcp must not be allowed", result.stderr)

    def test_apply_requires_explicit_confirmation(self) -> None:
        result = self.run_script("apply")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("apply requires --confirm", result.stderr)
        self.assertFalse(self.call_log.exists())

    def test_apply_adds_private_profile_rules_and_validates(self) -> None:
        result = self.run_script("apply", confirm=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.call_log.read_text(encoding="utf-8")
        self.assertIn("allow 22/tcp comment USDB operator SSH", calls)
        self.assertIn("allow 31303/tcp comment USDB devp2p TCP", calls)
        self.assertIn("allow 31303/udp comment USDB devp2p discovery", calls)
        self.assertNotIn("allow 8333/tcp", calls)
        self.assertIn("default deny incoming", calls)
        self.assertIn("--force enable", calls)

    def test_apply_removes_bitcoin_rule_when_switching_to_private(self) -> None:
        self.write_status(bitcoin_public=True)

        result = self.run_script("apply", confirm=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.call_log.read_text(encoding="utf-8")
        self.assertIn("--force delete allow 8333/tcp", calls)

    def test_apply_adds_public_bitcoin_rule(self) -> None:
        self.write_node_env(bitcoin_bind="0.0.0.0")
        self.write_status(bitcoin_public=True)

        result = self.run_script("apply", bitcoin_p2p="public", confirm=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.call_log.read_text(encoding="utf-8")
        self.assertIn("allow 8333/tcp comment Bitcoin P2P", calls)


if __name__ == "__main__":
    unittest.main()
