from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Reading:
    free_gb: float
    busy: float


def read_gpu(root=Path("/sys/class/drm")):
    readings = []
    for device in sorted(root.glob("card[0-9]*/device")):
        try:
            total = int((device / "mem_info_vram_total").read_text())
            used = int((device / "mem_info_vram_used").read_text())
            busy = int((device / "gpu_busy_percent").read_text())
            if total > 0 and 0 <= used <= total and 0 <= busy <= 100:
                readings.append((total, Reading((total - used) / 1e9, busy)))
        except (OSError, ValueError):
            continue
    # On the target host this selects the discrete Radeon, not an integrated GPU.
    return max(readings, key=lambda pair: pair[0])[1] if readings else None


class TierPolicy:
    """Decide when the GPU tier may hold the card, and when it must let go.

    Only VRAM decides whether the tier may load. Busy percent is a bad gate for
    that: a DCS server idling, or a compositor on its own, keeps the card at 10-30 %
    with spikes past any low bar, so a rule like "below 20 % for sixty consecutive
    seconds" is never satisfied on a desktop and the tier would never load at all.
    A 105 ms request interleaving with someone else's work is fine; taking VRAM they
    need is not. Busy percent still decides when to *let go*, which is the half that
    protects a game's frame time.

    Reclaiming is patient and the first load is not: waiting a minute at login only
    buys an initial index on the CPU, which takes eleven minutes instead of nine
    seconds.
    """

    def __init__(self, disabled=False, started=False):
        self.disabled = disabled
        self.wants_gpu = False
        self.busy_since = None
        self.calm_since = None
        self.started = started

    def step(self, now, reading, idle):
        if self.disabled or reading is None:
            return self.reset()
        if self.wants_gpu:
            self.calm_since = None
            if idle and reading.busy >= 50:
                if self.busy_since is None:
                    self.busy_since = now
            else:
                self.busy_since = None
            if reading.free_gb < 6 or (self.busy_since is not None and now - self.busy_since >= 5):
                return self.reset()
        else:
            self.busy_since = None
            if reading.free_gb >= 8:
                if self.calm_since is None:
                    self.calm_since = now
                # The first load happens as soon as the card has room; only a
                # reload after yielding waits out the settling period.
                if not self.started or now - self.calm_since >= 60:
                    self.wants_gpu = True
                    self.started = True
                    self.calm_since = None
                    return "load"
            else:
                self.calm_since = None
        return None

    def reset(self):
        # Yielding counts as having started: the next load is a reclaim and waits.
        self.started = self.started or self.wants_gpu
        action = "unload" if self.wants_gpu else None
        self.wants_gpu = False
        self.calm_since = self.busy_since = None
        return action
