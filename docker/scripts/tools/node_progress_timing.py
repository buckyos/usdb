"""Bounded, display-only elapsed times and rolling synchronization estimates."""

from collections import deque
from dataclasses import dataclass, field
from datetime import datetime
import math
from typing import Any


def service_elapsed(started_at: str, observed_at: str) -> int | None:
    """Use the current process start, never container creation or chain timestamps."""
    try:
        start = datetime.fromisoformat(started_at.replace("Z", "+00:00"))
        observed = datetime.fromisoformat(observed_at.replace("Z", "+00:00"))
        elapsed = (observed - start).total_seconds()
        if start.year > 1970 and start.tzinfo and observed.tzinfo and elapsed >= 0:
            return int(elapsed)
    except (TypeError, ValueError, AttributeError):
        pass
    return None


@dataclass
class _Samples:
    identity: tuple[Any, ...]
    started: float
    advanced: float
    points: deque[tuple[float, float, float]] = field(default_factory=lambda: deque(maxlen=128))


class ProgressTiming:
    """Estimate only fresh progress; never promote a service's readiness state."""

    WINDOW_SECS = 300
    MIN_SAMPLE_SECS = 30
    STALL_SECS = 60
    ACTIVE_STATES = {"SYNCING", "STARTING", "INSTALLING", "VERIFYING", "IMPORTING"}

    def __init__(self) -> None:
        self.started: float | None = None
        self.samples: dict[str, _Samples] = {}

    @staticmethod
    def _measure(component: dict[str, Any]) -> tuple[float, float, str] | None:
        verification = component.get("verification_progress")
        if component["id"] == "bitcoin" and ProgressTiming._number(verification) and 0 <= verification <= 1:
            return float(verification), 1.0, "verification"
        current, total = component.get("current"), component.get("total")
        if all(ProgressTiming._number(v) for v in (current, total)) and 0 <= current <= total and total > 0:
            return float(current), float(total), component.get("unit", "blocks")
        return None

    @staticmethod
    def _number(value: Any) -> bool:
        return isinstance(value, (float, int)) and not isinstance(value, bool) and math.isfinite(value)

    def apply(self, report: dict[str, Any], now: float) -> dict[str, Any]:
        """Return a copy; restart, phase changes and regressions reset the estimator."""
        if self.started is None or now < self.started:
            self.started = now
            self.samples.clear()
        components = []
        for original in report["components"]:
            component = dict(original)
            components.append(component)
            key = component["id"]
            state = component["state"]
            # Snapshot import already publishes authoritative attempt/stage timing.
            if state not in self.ACTIVE_STATES or "stage_elapsed_secs" in component:
                self.samples.pop(key, None)
                continue
            measure = self._measure(component)
            identity = (report.get("release_id"), report.get("network_bundle_id"),
                        report.get("resources", {}).get("phase"), component.get("progress_phase"),
                        component.get("startup_phase"), component.get("service_started_at"),
                        measure[2] if measure else None)
            sample = self.samples.get(key)
            if sample is None or sample.identity != identity or (sample.points and (
                now <= sample.points[-1][0] or now - sample.points[-1][0] > self.WINDOW_SECS
                or (measure and (measure[0] < sample.points[-1][1] or measure[1] < sample.points[-1][2]))
            )):
                sample = _Samples(identity, now, now)
                self.samples[key] = sample
            timing = {"elapsed_secs": component.get("service_elapsed_secs", int(now - sample.started)),
                      "elapsed_source": "process" if "service_elapsed_secs" in component else "observation",
                      "eta_secs": None, "eta_state": "sampling", "eta_basis": measure[2] if measure else None}
            component["timing"] = timing
            # STARTING may contain retained STALE heights; never sample those.
            if measure is None or state == "STARTING":
                timing["eta_state"] = "unavailable"
                sample.points.clear()
                sample.advanced = now
                continue
            current, total, basis = measure
            if sample.points and current > sample.points[-1][1]:
                sample.advanced = now
            sample.points.append((now, current, total))
            while sample.points and now - sample.points[0][0] > self.WINDOW_SECS:
                sample.points.popleft()
            if current >= total:
                timing["eta_state"] = "waiting-readiness"
            elif now - sample.advanced >= self.STALL_SECS:
                timing["eta_state"] = "stalled"
            elif len(sample.points) >= 3 and now - sample.points[0][0] >= self.MIN_SAMPLE_SECS:
                start, previous, previous_total = sample.points[0]
                duration = now - start
                rate = (current - previous) / duration
                # A moving upstream target can outpace local synchronization.
                closing_rate = ((previous_total - previous) - (total - current)) / duration
                timing["rate_per_sec"] = rate
                timing["sample_secs"] = duration
                if rate > 0 and closing_rate > 0:
                    timing.update(eta_secs=math.ceil((total - current) / closing_rate), eta_state="estimated")
                else:
                    timing["eta_state"] = "not-catching-up"
        present = {component["id"] for component in components}
        self.samples = {key: value for key, value in self.samples.items() if key in present}
        return {**report, "observation_elapsed_secs": int(now - self.started), "components": components}
