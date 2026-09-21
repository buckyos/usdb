"""Deterministic Core and Ord observations; no live Bitcoin node or wallet."""

from types import SimpleNamespace


def disk_space(free_bytes=400 * 1024**3):
    """Give Ord admission tests deterministic capacity independent of the runner."""
    return SimpleNamespace(free=free_bytes)


def core_observation(*, complete=True, indexed=100, synced=True):
    info = dict(chain="main", pruned=False, blocks=100, headers=100,
                bestblockhash="a" * 64, initialblockdownload=False, time=1000)
    active = dict(blocks=100, bestblockhash="a" * 64, validated=complete)
    chains = dict(chainstates=[active] if complete else
                  [dict(blocks=40, bestblockhash="b" * 64, validated=True), active])
    indexes = {"txindex": dict(best_block_height=indexed, synced=synced)} if indexed is not None else {}
    return info, chains, indexes


class Child:
    """Capture graceful subprocess shutdown without starting Ord."""
    def __init__(self):
        self.code = None
        self.signals = []

    def poll(self):
        return self.code

    def send_signal(self, value):
        self.signals.append(value)

    def wait(self):
        self.code = 0


class Loop:
    """Stop a supervisor deterministically after a bounded number of probes."""
    def __init__(self, cycles):
        self.cycles = cycles

    def is_set(self):
        return self.cycles <= 0

    def set(self):
        self.cycles = 0

    def wait(self, _seconds):
        self.cycles -= 1
