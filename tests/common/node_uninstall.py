"""Model only scoped systemd inspection and disable; never operate on host units."""
from pathlib import Path


class UninstallUnits:
    def __init__(self, plan, state="enabled"):
        self.paths = {Path(path).name: Path(path) for path in plan["units"]}
        self.states = {name: dict(LoadState="loaded", ActiveState="inactive", UnitFileState=state,
                                 FragmentPath=str(path), DropInPaths="") for name, path in self.paths.items()}
        self.commands = []
        self.fail_unit = None
        self.no_effect = False
        self.interrupt_after_disable = None
        self.runtime_enabled = set()
        self.before_disable = lambda unit: None

    def command(self, args, **kwargs):
        self.commands.append(list(args))
        if args[:2] == ["systemctl", "show"]:
            name = args[2]
            values = self.states[name] if self.paths[name].exists() else dict(LoadState="not-found", ActiveState="inactive", UnitFileState="")
            return "".join(f"{key}={value}\n" for key, value in values.items())
        if args[:2] == ["systemctl", "disable"]:
            name = args[-1]
            assert name in self.states and args[-2] == "--" and "--now" not in args
            self.before_disable(name)
            if name == self.fail_unit:
                raise ValueError("systemctl permission failure")
            if not self.no_effect:
                self.states[name]["UnitFileState"] = ("enabled-runtime" if name in self.runtime_enabled
                                                     and "--runtime" not in args else "disabled")
            if name == self.interrupt_after_disable:
                self.interrupt_after_disable = None
                raise KeyboardInterrupt()
            return ""
        if args == ["systemctl", "daemon-reload"]:
            return ""
        raise AssertionError(f"Unexpected host command: {args}")

    @property
    def disabled(self):
        return [args[-1] for args in self.commands if args[:2] == ["systemctl", "disable"]]
