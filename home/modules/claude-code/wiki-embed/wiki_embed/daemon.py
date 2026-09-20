from contextlib import contextmanager
from dataclasses import dataclass, field
import fcntl
import logging
import os
from pathlib import Path
import signal
import socket
import socketserver
import sqlite3
import threading
import time

from .core import (CPU_MODEL, Snapshot, Thresholds, cache_path, load_snapshot,
                   read_catalog, refresh_snapshot, retrieve, save_snapshot)
from .models import Models
from .policy import TierPolicy, read_gpu
from .protocol import MAX_LINE, decode_request, encode_message

LOG = logging.getLogger("wiki-embed")


@dataclass
class Tier:
    name: str
    model: object
    snapshot: Snapshot = field(default_factory=Snapshot)
    cancelled: threading.Event = field(default_factory=threading.Event)
    users: int = 0
    indexing: int = 0


class Daemon:
    def __init__(self, root, factory=Models, poll=read_gpu, no_gpu=False, thresholds=None):
        self.root = Path(root).resolve()
        self.factory = factory
        self.poll = poll
        self.policy = TierPolicy(no_gpu)
        self.thresholds = thresholds or Thresholds.from_env()
        self.lock = threading.Condition()
        # Transformers' loader temporarily changes torch's process-wide default
        # dtype. Serialize constructors, without making inference take this lock.
        self.model_load_lock = threading.Lock()
        self.stopped = threading.Event()
        self.policy_ready = threading.Event()
        self.gpu_attempted = threading.Event()
        self.gpu_wanted = threading.Event()
        self.cpu = self.gpu = self.gpu_loading = None
        self.snapshot = Snapshot()
        self.reading = None
        self.gpu_work = 0
        self.last_gpu_work = float("-inf")
        self.gpu_failed = False
        self.threads = []

    def start(self):
        for target in (self._monitor, self._cpu_worker, self._gpu_worker, self._index_worker):
            thread = threading.Thread(target=target, daemon=True, name=target.__name__)
            self.threads.append(thread)
            thread.start()

    def stop(self):
        self.stopped.set()
        with self.lock:
            for tier in (self.cpu, self.gpu, self.gpu_loading):
                if tier:
                    tier.cancelled.set()
            self.lock.notify_all()
        # Workers are daemonic because a native model load cannot be interrupted.
        for thread in self.threads:
            thread.join(timeout=1)

    @contextmanager
    def _activity(self, gpu):
        if gpu:
            with self.lock:
                self.gpu_work += 1
        try:
            yield
        finally:
            if gpu:
                with self.lock:
                    self.gpu_work -= 1
                    self.last_gpu_work = time.monotonic()

    def _withdraw_gpu(self):
        # A lease lets in-flight inference finish without holding the state lock.
        # New requests switch to CPU immediately; destruction happens on its worker.
        with self.lock:
            for tier in (self.gpu, self.gpu_loading):
                if tier:
                    tier.cancelled.set()
            self.gpu = None
            self.gpu_wanted.clear()
            self.lock.notify_all()

    def _monitor(self):
        last_poll = time.monotonic()
        while not self.stopped.is_set():
            try:
                reading = None if self.policy.disabled else self.poll()
                now = time.monotonic()
                with self.lock:
                    self.reading = reading
                    idle = self.gpu_work == 0 and self.last_gpu_work < last_poll
                    failed = self.gpu_failed
                    self.gpu_failed = False
                # Count only intervals without our own work as external GPU load.
                if failed:
                    self.policy.reset()
                    self._withdraw_gpu()
                action = self.policy.step(now, reading, idle)
                if action == "load":
                    LOG.info("GPU memory policy permits loading")
                    with self.lock:
                        self.gpu_wanted.set()
                        self.lock.notify_all()
                elif action == "unload":
                    LOG.info("Yielding GPU to the display")
                    self._withdraw_gpu()
                last_poll = now
            except Exception:
                LOG.exception("GPU poll failed")
                self.policy.reset()
                self._withdraw_gpu()
            finally:
                self.policy_ready.set()
            self.stopped.wait(2)

    def _cached(self, model):
        try:
            return load_snapshot(cache_path(self.root, model), model)
        except FileNotFoundError:
            return Snapshot()
        except Exception:
            LOG.warning("Ignoring unreadable vector cache for %s", model, exc_info=True)
            return Snapshot()

    def _publish(self, tier, snapshot):
        with self.lock:
            if self.stopped.is_set() or tier.cancelled.is_set():
                return
            tier.snapshot = snapshot
            if tier.name == "cpu":
                self.cpu = tier
            elif self.gpu_wanted.is_set():
                self.gpu = tier
            self.lock.notify_all()

    def _publish_snapshot(self, snapshot):
        with self.lock:
            self.snapshot = snapshot
            for tier in (self.cpu, self.gpu):
                if tier:
                    tier.snapshot = snapshot

    def _interrupted(self, tier):
        return (self.stopped.is_set() or tier.cancelled.is_set()
                or (tier.name == "cpu" and self.gpu_wanted.is_set()))

    def _refresh(self, tier, previous):
        pages = read_catalog(self.root)
        current = previous.retain(pages)
        self._publish_snapshot(current)
        if previous.indexed_at or not pages:
            self._publish(tier, current)
        if tuple(e.page for e in current.entries) == pages and previous.indexed_at:
            return current

        def encode(texts):
            # Yield between batches to already-running queries, never the reverse.
            with self.lock:
                while tier.users and not self._interrupted(tier):
                    self.lock.wait(timeout=0.1)
            if self._interrupted(tier):
                raise InterruptedError("refresh cancelled")
            with self._activity(tier.name == "gpu"):
                return tier.model.encode_passages(texts)

        last_save = time.monotonic()

        def progress(snapshot):
            nonlocal last_save
            # Recheck security flags even when publishing a partially built index.
            snapshot = snapshot.retain(read_catalog(self.root))
            self._publish_snapshot(snapshot)
            self._publish(tier, snapshot)
            if time.monotonic() - last_save >= 5:
                save_snapshot(cache_path(self.root, CPU_MODEL), CPU_MODEL, snapshot)
                LOG.info("%s index progress: %d/%d pages, %d chunks", tier.name,
                         len(snapshot.entries), len(pages), len(snapshot.rows))
                last_save = time.monotonic()

        try:
            updated = refresh_snapshot(self.root, pages, current, encode,
                                       lambda: self._interrupted(tier), progress)
            progress(updated)
            LOG.info("%s index refreshed: %d pages, %d chunks", tier.name,
                     len(self.snapshot.entries), len(self.snapshot.rows))
            return self.snapshot
        finally:
            # There is one writer, including at handoff. Preserve completed pages
            # on cancellation so neither a switch nor a restart duplicates them.
            if self.snapshot.indexed_at:
                save_snapshot(cache_path(self.root, CPU_MODEL), CPU_MODEL, self.snapshot)

    def _index_worker(self):
        cached = self._cached(CPU_MODEL)
        initialized = False
        while not self.stopped.is_set():
            tier = None
            waiting_for_model = False
            try:
                if not self.policy_ready.wait(0.2):
                    continue
                pages = read_catalog(self.root)
                self._publish_snapshot((self.snapshot if initialized else cached).retain(pages))
                initialized = True
                with self.lock:
                    # A cold CPU index must not race the GPU loader. Model readiness
                    # is independent of indexing, so this wait has no circular gate.
                    tier = self.gpu if self.gpu_wanted.is_set() else self.cpu
                    waiting_for_model = tier is None
                    if tier is not None:
                        tier.indexing += 1
                if tier is not None:
                    self._refresh(tier, self.snapshot)
            except InterruptedError:
                continue
            except (OSError, ValueError, sqlite3.Error) as error:
                LOG.warning("Index refresh: %s", error)
            except Exception:
                LOG.exception("Index inference failed")
                if tier and tier.name == "gpu":
                    with self.lock:
                        self.gpu_failed = True
                    self._withdraw_gpu()
            finally:
                if tier is not None:
                    with self.lock:
                        tier.indexing -= 1
                        self.lock.notify_all()
            with self.lock:
                if not self.stopped.is_set():
                    if waiting_for_model:
                        self.lock.wait_for(
                            lambda: self.stopped.is_set() or
                            (self.gpu if self.gpu_wanted.is_set() else self.cpu) is not None,
                            timeout=5)
                    else:
                        self.lock.wait(timeout=5)

    def _serve_tier(self, tier):
        with self.lock:
            self._publish(tier, self.snapshot)
        LOG.info("%s models ready", tier.name)
        while not self.stopped.is_set() and not tier.cancelled.wait(0.2):
            pass

    def _dispose(self, tier):
        with self.lock:
            while tier.users or tier.indexing:
                self.lock.wait(timeout=0.1)
        tier.model.close()

    def _cpu_worker(self):
        while not self.stopped.is_set():
            if not self.policy_ready.wait(0.2):
                continue
            # Give an immediately available GPU the first constructor slot too;
            # its loader never depends on a CPU model or on a completed index.
            if self.gpu_wanted.is_set() and not self.gpu_attempted.is_set():
                self.stopped.wait(0.1)
                continue
            tier = None
            try:
                LOG.info("Loading CPU models")
                with self.model_load_lock:
                    tier = Tier("cpu", self.factory("cpu"))
                self._serve_tier(tier)
            except Exception:
                LOG.exception("CPU tier failed; retrying in 30 seconds")
            finally:
                with self.lock:
                    self.cpu = None
                if tier:
                    self._dispose(tier)
            self.stopped.wait(30)

    def _gpu_worker(self):
        while not self.stopped.wait(0.2):
            if not self.gpu_wanted.is_set():
                continue
            tier = None
            try:
                LOG.info("Loading GPU models")
                try:
                    with self.model_load_lock:
                        if not self.gpu_wanted.is_set() or self.stopped.is_set():
                            continue
                        with self._activity(True):
                            model = self.factory("gpu")
                finally:
                    self.gpu_attempted.set()
                tier = Tier("gpu", model)
                with self.lock:
                    self.gpu_loading = tier
                    if not self.gpu_wanted.is_set():
                        tier.cancelled.set()
                self._serve_tier(tier)
            except Exception:
                LOG.exception("GPU tier failed; falling back to CPU")
                with self.lock:
                    self.gpu_failed = True
                self._withdraw_gpu()
            finally:
                with self.lock:
                    self.gpu = self.gpu_loading = None
                if tier:
                    self._dispose(tier)

    def handle(self, request):
        if request["op"] == "status":
            with self.lock:
                tier = self.gpu or self.cpu
                snapshot = tier.snapshot if tier else self.snapshot
                reading = self.reading
                return {"ok": True, "tier": tier.name if tier else "cpu",
                        "pages": len(snapshot.entries), "chunks": len(snapshot.rows),
                        "indexed_at": snapshot.indexed_at,
                        "vram_free_gb": reading.free_gb if reading else 0.0,
                        "gpu_busy": reading.busy if reading else 0.0,
                        "model": tier.model.name if tier else CPU_MODEL}
        for name in ("gpu", "cpu"):
            with self.lock:
                tier = getattr(self, name)
                if tier is None:
                    continue
                snapshot = tier.snapshot
                if not snapshot.indexed_at:
                    continue
                tier.users += 1
            try:
                with self._activity(name == "gpu"):
                    results = retrieve(snapshot, tier.model, name,
                                       request.get("prompt", request.get("query")), request["op"],
                                       request["n"], self.thresholds)
                return {"ok": True, "tier": name, "results": results}
            except Exception:
                if name == "cpu":
                    raise
                LOG.exception("GPU request failed; retrying on CPU")
                with self.lock:
                    self.gpu_failed = True
                self._withdraw_gpu()
            finally:
                with self.lock:
                    tier.users -= 1
                    self.lock.notify_all()
        raise RuntimeError("models/index are warming up; retry shortly")


class Handler(socketserver.StreamRequestHandler):
    timeout = 2

    def handle(self):
        try:
            request = decode_request(self.rfile.readline(MAX_LINE + 1))
            response = self.server.daemon.handle(request)
        except Exception as error:
            response = {"ok": False, "error": str(error)}
        try:
            self.wfile.write(encode_message(response))
        except (BrokenPipeError, ConnectionResetError, socket.timeout):
            pass


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True

    def __init__(self, path, daemon):
        self.daemon = daemon
        super().__init__(str(path), Handler)


def default_socket():
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime:
        raise ValueError("XDG_RUNTIME_DIR is not set")
    return Path(runtime) / "claude-wiki" / "embed.sock"


def serve(root, path, no_gpu):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    # A lock distinguishes a stale socket from a second live daemon at login.
    with (path.parent / "embed.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        path.unlink(missing_ok=True)
        daemon = Daemon(root, no_gpu=no_gpu)
        try:
            with Server(path, daemon) as server:
                path.chmod(0o600)
                server.timeout = 0.5
                signal.signal(signal.SIGTERM, lambda *_: daemon.stopped.set())
                signal.signal(signal.SIGINT, lambda *_: daemon.stopped.set())
                daemon.start()
                while not daemon.stopped.is_set():
                    server.handle_request()
        finally:
            daemon.stop()
            path.unlink(missing_ok=True)
