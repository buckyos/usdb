"""Deterministic elapsed time for long snapshot retry windows; no real sleeping."""


class DownloadClock:
    def __init__(self):
        self.elapsed = 0
        self.sleeps = []
        self.on_sleep = lambda: None

    def monotonic(self):
        return self.elapsed

    def time(self):
        return 1790867259 + self.elapsed

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.elapsed += seconds
        self.on_sleep()
