from dataclasses import replace
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import numpy as np

from wiki_embed.core import (CPU_MODEL, GPU_MODEL, Chunk, Entry, Page, Section, Snapshot,
                             Thresholds, cache_path, chunk_page, load_snapshot, read_catalog,
                             refresh_snapshot, retrieve, save_snapshot, section_for_line)
from wiki_embed.daemon import Daemon, Server, Tier
from wiki_embed.policy import Reading, TierPolicy, read_gpu
from wiki_embed.protocol import MAX_LINE, decode_request, encode_message


class FakeModel:
    def __init__(self, tier="cpu"):
        self.name = GPU_MODEL if tier == "gpu" else CPU_MODEL
        self.encoded = []
        self.reranked = []
        self.closed = False

    def encode_passages(self, texts):
        self.encoded.extend(texts)
        return np.tile([1.0, 0.0], (len(texts), 1)).astype(np.float32)

    def encode_query(self, query):
        return np.array([1.0, 0.0], dtype=np.float32)

    def rerank(self, query, texts):
        self.reranked = texts
        return np.arange(len(texts), dtype=np.float32) - 0.2

    def close(self):
        self.closed = True


def example_snapshot(scores=(0.8, 0.7), sections=()):
    return Snapshot([Entry(Page(f"pages/knowledge/p{i}.md", 10, 1, sections),
                           (Chunk(f"passage {i}", 27),), np.array([[s, 0]], dtype=np.float32))
                     for i, s in enumerate(scores)], 1234.0)


class ChunkTests(unittest.TestCase):
    def test_exact_windows_overlap_and_no_redundant_tail(self):
        for count, sizes in ((1, [1]), (180, [180]), (181, [180, 41]),
                             (320, [180, 180]), (321, [180, 180, 41])):
            with self.subTest(count=count):
                text = "---\ntitle: Example\ntags: [one, two]\nproject: hidden\n---\n\n"
                text += "\n".join(f"word{i}" for i in range(count))
                chunks = chunk_page(text, "knowledge/fallback")
                prefix = "Example. one, two. "
                windows = [c.text.removeprefix(prefix).split() for c in chunks]
                self.assertEqual([len(w) for w in windows], sizes)
                for i, chunk in enumerate(chunks):
                    self.assertTrue(chunk.text.startswith(prefix))
                    self.assertNotIn("hidden", chunk.text)
                    self.assertEqual(chunk.first_line, 7 + i * 140)
                for first, second in zip(windows, windows[1:]):
                    self.assertEqual(first[-40:], second[:40])

    def test_unicode_whitespace_and_original_line_numbers(self):
        chunks = chunk_page("---\r\ntitle: 한국어\r\n---\r\n\r\n  첫째\t둘째\r\n\r\n셋째", "x")
        self.assertEqual(chunks, (Chunk("한국어. 첫째 둘째 셋째", 5),))

    def test_empty_body_has_only_metadata(self):
        self.assertEqual(chunk_page("---\ntitle: Example\n---\n", "x"),
                         (Chunk("Example.", 4),))

    def test_no_frontmatter_or_unterminated_frontmatter(self):
        self.assertEqual(chunk_page("\nbody", "knowledge/x"),
                         (Chunk("knowledge/x. body", 2),))
        self.assertIn("--- title: still body", chunk_page("---\ntitle: still body", "x")[0].text)

    def test_section_boundaries_and_missing_sections(self):
        sections = (Section("First", 7, 26), Section("What kills it", 27, 36))
        for line in (27, 30, 36):
            self.assertEqual(section_for_line(sections, line), sections[1])
        self.assertEqual(section_for_line(sections, 26), sections[0])
        self.assertIsNone(section_for_line(sections, 6))
        self.assertIsNone(section_for_line(sections, 37))
        self.assertIsNone(section_for_line((), 27))

    def test_winning_chunk_cites_its_first_line_not_later_heading(self):
        sections = (Section("What kills it", 27, 36), Section("Later", 37, 90))
        entry = Entry(Page("pages/knowledge/foo.md", 1, 0, sections),
                      (Chunk("first", 10), Chunk("second crosses next heading", 27)),
                      np.array([[0.1, 0], [0.9, 0]], dtype=np.float32))
        result = retrieve(Snapshot([entry]), FakeModel(), "cpu", "q", "search", 10, Thresholds())
        self.assertEqual(result[0]["path"], "knowledge/foo")
        self.assertEqual(result[0]["heading"], "What kills it")
        self.assertEqual((result[0]["start_line"], result[0]["end_line"]), (27, 36))


class PolicyTests(unittest.TestCase):
    def loaded(self):
        """A fresh policy takes the card as soon as it has room, with no wait."""
        policy = TierPolicy()
        self.assertEqual(policy.step(0, Reading(8, 19), True), "load")
        return policy

    def test_first_load_is_immediate_and_ignores_busy(self):
        # Busy percent must not gate loading: a game server or a compositor sits at
        # 10-30 % with spikes, so any low bar would keep the tier from ever loading.
        self.assertEqual(TierPolicy().step(0, Reading(8, 95), True), "load")
        # Not enough memory is still a hard no, however quiet the card is.
        policy = TierPolicy()
        self.assertIsNone(policy.step(0, Reading(7.99, 0), True))
        self.assertEqual(policy.step(1, Reading(8, 0), True), "load")

    def test_low_memory_unloads_even_during_own_work(self):
        policy = self.loaded()
        self.assertIsNone(policy.step(61, Reading(6, 0), False))
        self.assertEqual(policy.step(62, Reading(5.99, 0), False), "unload")
        self.assertIsNone(policy.step(63, Reading(7.99, 0), True))
        self.assertIsNone(policy.step(64, Reading(8, 0), True))
        self.assertIsNone(policy.step(123.99, Reading(8, 0), True))
        self.assertEqual(policy.step(124, Reading(8, 0), True), "load")

    def test_busy_alone_never_blocks_a_reload(self):
        policy = self.loaded()
        self.assertEqual(policy.step(1, Reading(5, 0), True), "unload")
        for now in (2, 30, 60):
            self.assertIsNone(policy.step(now, Reading(20, 100), False))
        self.assertEqual(policy.step(62, Reading(20, 100), False), "load")

    def test_busy_for_five_seconds_only_while_idle(self):
        policy = self.loaded()
        self.assertIsNone(policy.step(61, Reading(10, 50), True))
        self.assertIsNone(policy.step(65.99, Reading(10, 50), True))
        self.assertEqual(policy.step(66, Reading(10, 50), True), "unload")

    def test_own_work_and_low_busy_reset_busy_timer(self):
        policy = self.loaded()
        for now, busy, idle in ((61, 90, True), (65, 90, False), (70, 90, True),
                                (74, 49, True), (80, 50, True), (84, 50, True)):
            self.assertIsNone(policy.step(now, Reading(10, busy), idle))
        self.assertEqual(policy.step(85, Reading(10, 50), True), "unload")

    def test_reload_after_yielding_waits_for_sustained_memory(self):
        policy = self.loaded()
        self.assertEqual(policy.step(1, Reading(5, 0), True), "unload")
        # Memory must stay available for a minute; a dip restarts the clock.
        for now, free in ((2, 10), (30, 7.9), (31, 10), (90, 10)):
            self.assertIsNone(policy.step(now, Reading(free, 95), True))
        self.assertEqual(policy.step(91, Reading(10, 95), True), "load")

    def test_two_second_polling_and_disabled_gpu(self):
        policy = self.loaded()
        for now in (62, 64, 66):
            self.assertIsNone(policy.step(now, Reading(10, 50), True))
        self.assertEqual(policy.step(68, Reading(10, 50), True), "unload")
        disabled = TierPolicy(True)
        for now in (0, 60, 600):
            self.assertIsNone(disabled.step(now, Reading(25, 0), True))
        self.assertFalse(disabled.wants_gpu)

    def test_missing_reading_unloads_and_restarts_cooldown(self):
        policy = self.loaded()
        self.assertEqual(policy.step(61, None, True), "unload")
        self.assertIsNone(policy.step(100, Reading(10, 0), True))
        self.assertIsNone(policy.step(159, Reading(10, 0), True))
        self.assertEqual(policy.step(160, Reading(10, 0), True), "load")

    def test_sysfs_decimal_gb_and_missing_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertIsNone(read_gpu(root))
            device = root / "card1/device"
            device.mkdir(parents=True)
            for key, value in (("mem_info_vram_total", 25_800_000_000),
                               ("mem_info_vram_used", 20_000_000_000), ("gpu_busy_percent", 54)):
                (device / key).write_text(str(value))
            self.assertEqual(read_gpu(root), Reading(5.8, 54))


class RetrievalTests(unittest.TestCase):
    def test_cpu_threshold_and_explicit_search(self):
        model = FakeModel()
        # Two near-identical scores are no longer a reason to stay quiet: rules that
        # normalise against the other candidates measured worse than the raw score.
        self.assertEqual([r["path"] for r in retrieve(example_snapshot((0.8, 0.79)), model, "cpu",
                                                      "q", "related", 1, Thresholds())],
                         ["knowledge/p0"])
        self.assertEqual(retrieve(example_snapshot((0.59, 0.2)), model, "cpu", "q", "related", 3,
                                  Thresholds()), [])
        result = retrieve(example_snapshot(), model, "cpu", "q", "related", 3, Thresholds())
        self.assertEqual([r["path"] for r in result], ["knowledge/p0", "knowledge/p1"])
        self.assertNotIn("heading", result[0])
        self.assertEqual(len(retrieve(example_snapshot((0.1, 0.09)), model, "cpu", "q", "search", 10,
                                     Thresholds())), 2)

    def test_single_page_empty_corpus_and_configured_thresholds(self):
        self.assertEqual(retrieve(Snapshot(), FakeModel(), "cpu", "q", "related", 1, Thresholds()), [])
        self.assertEqual(len(retrieve(example_snapshot((0.7,)), FakeModel(), "cpu", "q", "related", 1,
                                     Thresholds())), 1)
        with patch.dict(os.environ, {"WIKI_EMBED_RERANK_THRESHOLD": "-2",
                                    "WIKI_EMBED_COSINE_THRESHOLD": "0.9"}):
            self.assertEqual(Thresholds.from_env(), Thresholds(-2, 0.9))

    def test_gpu_ten_distinct_pages_raw_scores_and_sort_order(self):
        snapshot = example_snapshot(tuple(1 - i * 0.01 for i in range(12)))
        first = snapshot.entries[0]
        repeated = replace(first, chunks=first.chunks * 20, vectors=np.tile(first.vectors, (20, 1)))
        snapshot = Snapshot([repeated, *snapshot.entries[1:]])
        model = FakeModel("gpu")
        results = retrieve(snapshot, model, "gpu", "q", "related", 50, Thresholds())
        self.assertEqual(len(model.reranked), 10)
        self.assertEqual(len(set(model.reranked)), 10)
        self.assertEqual(len(results), 9)
        self.assertEqual(results[0]["path"], "knowledge/p9")
        self.assertGreater(results[0]["score"], 1)
        self.assertEqual(len(retrieve(snapshot, model, "gpu", "q", "search", 50, Thresholds())), 10)


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "pages/knowledge").mkdir(parents=True)
        self.path = self.root / "pages/knowledge/foo.md"
        self.path.write_text("---\ntitle: Test\ntags: [one, two]\n---\n## Heading\nbody")
        self.db = sqlite3.connect(self.root / "index.sqlite")
        self.addCleanup(self.db.close)
        self.db.executescript("CREATE TABLE files(id INTEGER PRIMARY KEY, path TEXT, mtime_ns INTEGER, size INTEGER, blocked TEXT);"
                              "CREATE TABLE sections(id INTEGER PRIMARY KEY, file_id INTEGER, heading TEXT, start_line INTEGER, end_line INTEGER);")
        self.db.execute("INSERT INTO files VALUES (1, ?, ?, ?, '')",
                        ("pages/knowledge/foo.md", self.path.stat().st_mtime_ns, self.path.stat().st_size))
        self.db.execute("INSERT INTO sections VALUES (1, 1, 'Heading', 5, 6)")
        self.db.commit()
        self.model = FakeModel()

    def refresh(self, previous=None):
        return refresh_snapshot(self.root, read_catalog(self.root), previous or Snapshot(),
                                self.model.encode_passages)

    def wait_for(self, predicate):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.01)
        self.fail("background work did not reach the expected state")

    def second_page(self):
        path = self.root / "pages/knowledge/second.md"
        path.write_text("the second page")
        self.db.execute("INSERT INTO files VALUES (2, ?, ?, ?, '')",
                        ("pages/knowledge/second.md", path.stat().st_mtime_ns, path.stat().st_size))
        self.db.commit()

    def start_daemon(self, factory, no_gpu=False):
        daemon = Daemon(self.root, factory=factory, poll=lambda: Reading(21.1, 25), no_gpu=no_gpu)
        daemon.start()
        self.addCleanup(daemon.stop)
        return daemon

    def test_gpu_serves_cold_index_before_cpu_model_is_ready(self):
        cpu_entered, cpu_release = threading.Event(), threading.Event()
        models = {}
        calls = []

        def factory(tier):
            calls.append(tier)
            if tier == "cpu":
                cpu_entered.set()
                cpu_release.wait(5)
            models[tier] = FakeModel(tier)
            return models[tier]

        daemon = self.start_daemon(factory)
        self.addCleanup(cpu_release.set)
        self.assertTrue(cpu_entered.wait(2))
        self.wait_for(lambda: daemon.handle({"op": "status"})["chunks"] == 1)
        self.assertEqual(calls[0], "gpu")
        self.assertIsNone(daemon.cpu)
        status = daemon.handle({"op": "status"})
        self.assertEqual((status["tier"], status["pages"]), ("gpu", 1))
        self.assertEqual(daemon.handle({"op": "search", "query": "q", "n": 1})["tier"], "gpu")
        self.assertEqual(len(models["gpu"].encoded), 1)

    def test_cold_index_waits_for_gpu_loader_and_is_not_duplicated(self):
        entered, release = threading.Event(), threading.Event()
        models = {}

        def factory(tier):
            if tier == "gpu":
                entered.set()
                release.wait(5)
            models[tier] = FakeModel(tier)
            return models[tier]

        daemon = self.start_daemon(factory)
        self.addCleanup(release.set)
        self.assertTrue(entered.wait(2))
        self.assertEqual(daemon.handle({"op": "status"})["chunks"], 0)
        self.assertFalse(any(m.encoded for m in models.values()))
        with self.assertRaisesRegex(RuntimeError, "warming up"):
            daemon.handle({"op": "related", "prompt": "q", "n": 1})
        release.set()
        self.wait_for(lambda: daemon.cpu is not None and daemon.gpu is not None
                      and len(daemon.snapshot.entries) == 1 and daemon.gpu.indexing == 0)
        self.assertEqual(len(models["gpu"].encoded), 1)
        self.assertEqual(models["cpu"].encoded, [])
        self.assertIs(daemon.cpu.snapshot, daemon.gpu.snapshot)
        # A later CPU-only start consumes the same cache without encoding anything.
        daemon.stop()
        restarted_models = []

        def restart_factory(tier):
            restarted_models.append(FakeModel(tier))
            return restarted_models[-1]

        restarted = self.start_daemon(restart_factory, no_gpu=True)
        self.wait_for(lambda: restarted.cpu is not None and len(restarted.cpu.snapshot.entries) == 1)
        self.assertEqual(restarted_models[0].encoded, [])

    def test_cold_cpu_index_publishes_completed_pages_and_serves_requests(self):
        self.second_page()
        entered, release = threading.Event(), threading.Event()
        model = FakeModel()
        original_encode = model.encode_passages

        def encode(texts):
            if "second page" in texts[0]:
                entered.set()
                release.wait(5)
            return original_encode(texts)

        model.encode_passages = encode
        daemon = self.start_daemon(lambda tier: model, no_gpu=True)
        self.addCleanup(release.set)
        self.assertTrue(entered.wait(2))
        status = daemon.handle({"op": "status"})
        self.assertEqual((status["pages"], status["chunks"]), (1, 1))
        self.assertGreater(status["indexed_at"], 0)
        start = time.monotonic()
        result = daemon.handle({"op": "related", "prompt": "q", "n": 1})
        self.assertLess(time.monotonic() - start, 0.5)
        self.assertEqual(result["results"][0]["path"], "knowledge/foo")
        release.set()
        self.wait_for(lambda: daemon.handle({"op": "status"})["pages"] == 2)

    def test_gpu_handoff_preserves_progress_and_has_only_one_index_writer(self):
        self.second_page()
        gpu_entered, gpu_release = threading.Event(), threading.Event()
        cpu_entered, cpu_release = threading.Event(), threading.Event()
        models = {}

        def factory(tier):
            model = FakeModel(tier)
            models[tier] = model
            original_encode = model.encode_passages

            def encode(texts):
                if "second page" in texts[0]:
                    if tier == "gpu":
                        gpu_entered.set()
                        gpu_release.wait(5)
                        raise InterruptedError("GPU yielded during this page")
                    cpu_entered.set()
                    cpu_release.wait(5)
                return original_encode(texts)

            model.encode_passages = encode
            return model

        daemon = self.start_daemon(factory)
        self.addCleanup(gpu_release.set)
        self.addCleanup(cpu_release.set)
        self.assertTrue(gpu_entered.wait(2))
        self.wait_for(lambda: daemon.cpu is not None)
        self.assertEqual(daemon.handle({"op": "status"})["pages"], 1)
        daemon._withdraw_gpu()
        self.assertFalse(models["gpu"].closed)
        self.assertFalse(cpu_entered.is_set())
        gpu_release.set()
        self.assertTrue(cpu_entered.wait(2))
        cached = load_snapshot(cache_path(self.root, CPU_MODEL), CPU_MODEL)
        self.assertEqual(len(cached.entries), 1)
        self.assertEqual(daemon.handle({"op": "search", "query": "q", "n": 1})["tier"], "cpu")
        cpu_release.set()
        self.wait_for(lambda: daemon.handle({"op": "status"})["pages"] == 2)
        self.assertEqual(len(models["gpu"].encoded), 1)
        self.assertEqual(len(models["cpu"].encoded), 1)
        self.assertIn("second page", models["cpu"].encoded[0])

    def test_failed_gpu_load_falls_back_to_cold_cpu_index(self):
        calls = []

        def factory(tier):
            calls.append(tier)
            if tier == "gpu":
                raise RuntimeError("GPU unavailable")
            return FakeModel(tier)

        with self.assertLogs("wiki-embed", level="ERROR"):
            daemon = self.start_daemon(factory)
            self.wait_for(lambda: daemon.handle({"op": "status"})["pages"] == 1)
        self.assertEqual(calls, ["gpu", "cpu"])
        self.assertEqual(daemon.handle({"op": "status"})["tier"], "cpu")

    def test_cache_restart_mtime_and_sections_only_change(self):
        first = self.refresh()
        self.assertEqual(len(self.model.encoded), 1)
        path = cache_path(self.root, CPU_MODEL)
        save_snapshot(path, CPU_MODEL, first)
        cached = load_snapshot(path, CPU_MODEL)
        self.assertEqual(cached.entries[0].chunks, first.entries[0].chunks)
        np.testing.assert_array_equal(cached.vectors, first.vectors)
        self.db.execute("UPDATE sections SET heading = 'Renamed'")
        self.db.commit()
        second = self.refresh(cached)
        self.assertEqual(len(self.model.encoded), 1)
        self.assertEqual(second.entries[0].page.sections[0].heading, "Renamed")
        self.path.write_text("changed")
        os.utime(self.path, ns=(1, first.entries[0].page.mtime_ns + 1))
        self.db.execute("UPDATE files SET mtime_ns = ?, size = ?",
                        (self.path.stat().st_mtime_ns, self.path.stat().st_size))
        self.db.commit()
        self.refresh(second)
        self.assertEqual(len(self.model.encoded), 2)

    def test_blocked_and_deleted_pages_are_removed_without_embedding(self):
        original = self.refresh()
        self.db.execute("UPDATE files SET blocked = 'security finding'")
        self.db.commit()
        self.assertEqual(read_catalog(self.root), ())
        self.assertEqual(len(original.retain(read_catalog(self.root)).entries), 0)
        self.assertEqual(len(self.refresh(original).entries), 0)
        self.db.execute("DELETE FROM files")
        self.db.commit()
        self.assertEqual(len(self.refresh(original).entries), 0)
        self.assertEqual(len(self.model.encoded), 1)

    def test_unscanned_edit_is_not_cached_under_old_mtime(self):
        self.path.write_text("new bytes that the CLI has not scanned yet")
        self.assertEqual(len(self.refresh().entries), 0)
        self.assertEqual(self.model.encoded, [])

    def test_read_only_missing_database_is_not_created(self):
        other = self.root / "absent"
        other.mkdir()
        with self.assertRaises(sqlite3.OperationalError):
            read_catalog(other)
        self.assertFalse((other / "index.sqlite").exists())

    def test_cache_model_mismatch_and_cancelled_refresh(self):
        path = cache_path(self.root, CPU_MODEL)
        save_snapshot(path, CPU_MODEL, self.refresh())
        with self.assertRaises(ValueError):
            load_snapshot(path, "some/other-encoder")
        with self.assertRaises(InterruptedError):
            refresh_snapshot(self.root, read_catalog(self.root), Snapshot(),
                             self.model.encode_passages, lambda: True)

    def test_symlink_outside_pages_is_skipped(self):
        target = self.root / "private.md"
        target.write_text(self.path.read_text())
        self.path.unlink()
        self.path.symlink_to(target)
        self.db.execute("UPDATE files SET mtime_ns = ?, size = ?",
                        (target.stat().st_mtime_ns, target.stat().st_size))
        self.db.commit()
        self.assertEqual(len(self.refresh().entries), 0)

    def test_background_refresh_does_not_block_requests(self):
        daemon = Daemon(self.root, factory=FakeModel, no_gpu=True)
        previous = self.refresh()
        second = self.root / "pages/knowledge/second.md"
        second.write_text("a new page")
        self.db.execute("INSERT INTO files VALUES (2, ?, ?, ?, '')",
                        ("pages/knowledge/second.md", second.stat().st_mtime_ns, second.stat().st_size))
        self.db.commit()
        entered, release = threading.Event(), threading.Event()
        model = FakeModel()

        def slow_encode(texts):
            entered.set()
            if not release.wait(3):
                raise RuntimeError("test timed out")
            return np.tile([1.0, 0.0], (len(texts), 1))

        model.encode_passages = slow_encode
        tier = Tier("cpu", model)
        thread = threading.Thread(target=daemon._refresh, args=(tier, previous))
        thread.start()
        try:
            self.assertTrue(entered.wait(1))
            start = time.monotonic()
            response = daemon.handle({"op": "search", "query": "query", "n": 1})
            self.assertLess(time.monotonic() - start, 0.5)
            self.assertEqual(response["tier"], "cpu")
            self.assertEqual(response["results"][0]["path"], "knowledge/foo")
        finally:
            release.set()
            thread.join(3)
        self.assertEqual(len(tier.snapshot.rows), 2)

    def test_initial_refresh_does_not_report_false_empty_results(self):
        daemon = Daemon(self.root, factory=FakeModel, no_gpu=True)
        tier = Tier("cpu", FakeModel())

        def encode(texts):
            self.assertIsNone(daemon.cpu)
            return self.model.encode_passages(texts)

        tier.model.encode_passages = encode
        daemon._refresh(tier, Snapshot())
        self.assertIs(daemon.cpu, tier)

    def test_page_blocked_during_embedding_is_not_published(self):
        daemon = Daemon(self.root, factory=FakeModel, no_gpu=True)
        tier = Tier("cpu", FakeModel())

        def encode(texts):
            self.db.execute("UPDATE files SET blocked = 'new finding'")
            self.db.commit()
            return self.model.encode_passages(texts)

        tier.model.encode_passages = encode
        daemon._refresh(tier, Snapshot())
        self.assertEqual(tier.snapshot.entries, ())


class ProtocolTests(unittest.TestCase):
    def test_unicode_and_embedded_newlines_round_trip(self):
        request = {"op": "related", "prompt": '한국어\n"prompt"', "n": 3}
        encoded = encode_message(request)
        self.assertEqual(encoded.count(b"\n"), 1)
        self.assertEqual(decode_request(encoded), request)
        self.assertEqual(decode_request(b'{"op":"status"}\n'), {"op": "status"})
        self.assertEqual(decode_request(b'{"op":"search","query":"x"}\n')["n"], 10)

    def test_malformed_requests(self):
        cases = [b"{}\n", b"[]\n", b"null\n", b"not json\n", b"{}", b"\xff\n",
                 b'{"op":"search","query":3}\n', b'{"op":"search","query":" "}\n',
                 b'{"op":"related","prompt":"x","n":true}\n',
                 b'{"op":"related","prompt":"x","n":0}\n',
                 b'{"op":"related","prompt":"x","n":1.5}\n',
                 b'{"op":"status"}\n{}\n', b"x" * MAX_LINE + b"\n"]
        for line in cases:
            with self.subTest(line=line[:80]), self.assertRaises(ValueError):
                decode_request(line)

    def test_encoding_rejects_nonfinite_scores(self):
        with self.assertRaises(ValueError):
            encode_message({"score": float("nan")})

    def test_socket_one_request_per_connection_and_client(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "embed.sock"
            daemon = Daemon(directory, factory=FakeModel, no_gpu=True)
            daemon.cpu = Tier("cpu", FakeModel(), example_snapshot())
            with Server(path, daemon) as server:
                thread = threading.Thread(target=server.serve_forever)
                thread.start()
                try:
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                        connection.settimeout(2)
                        connection.connect(str(path))
                        connection.sendall(b'{"op":"status"}\n{"op":"status"}\n')
                        stream = connection.makefile("rb")
                        with stream:
                            status = json.loads(stream.readline())
                            self.assertEqual(stream.readline(), b"")
                        self.assertEqual(status["pages"], 2)
                        self.assertEqual(status["chunks"], 2)
                        self.assertEqual(status["model"], CPU_MODEL)
                        self.assertEqual(status["indexed_at"], 1234.0)
                        self.assertIn("vram_free_gb", status)
                        self.assertIn("gpu_busy", status)
                    client = subprocess.run([sys.executable, "-m", "wiki_embed", "--socket", str(path),
                                             "--client", '{"op":"search","query":"한국어","n":1}'],
                                            capture_output=True, text=True, timeout=5)
                    self.assertEqual(client.returncode, 0, client.stderr)
                    self.assertEqual(len(json.loads(client.stdout)["results"]), 1)
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                        connection.settimeout(2)
                        connection.connect(str(path))
                        connection.sendall(b"garbage\n")
                        self.assertFalse(json.loads(connection.recv(4096))["ok"])
                finally:
                    server.shutdown()
                    thread.join()

    def test_loading_gpu_and_unload_keep_cpu_available(self):
        daemon = Daemon("/tmp", factory=FakeModel)
        daemon.cpu = Tier("cpu", FakeModel(), example_snapshot())
        daemon.gpu_loading = Tier("gpu", FakeModel("gpu"))
        request = {"op": "search", "query": "q", "n": 1}
        self.assertEqual(daemon.handle(request)["tier"], "cpu")
        daemon.gpu = Tier("gpu", FakeModel("gpu"), example_snapshot())
        self.assertEqual(daemon.handle(request)["tier"], "gpu")
        old = daemon.gpu
        daemon._withdraw_gpu()
        self.assertTrue(old.cancelled.is_set())
        self.assertEqual(daemon.handle(request)["tier"], "cpu")

    def test_gpu_failure_falls_back_within_same_request(self):
        daemon = Daemon("/tmp", factory=FakeModel)
        daemon.cpu = Tier("cpu", FakeModel(), example_snapshot())
        daemon.gpu = Tier("gpu", FakeModel("gpu"), example_snapshot())
        with patch.object(daemon.gpu.model, "encode_query", side_effect=RuntimeError("out of memory")):
            with self.assertLogs("wiki-embed", level="ERROR"):
                result = daemon.handle({"op": "related", "prompt": "q", "n": 1})
        self.assertEqual(result["tier"], "cpu")
        self.assertIsNone(daemon.gpu)

    def test_gpu_unload_waits_only_for_existing_lease(self):
        daemon = Daemon("/tmp", factory=FakeModel)
        daemon.cpu = Tier("cpu", FakeModel(), example_snapshot())
        tier = Tier("gpu", FakeModel("gpu"), example_snapshot())
        daemon.gpu = tier
        entered, release = threading.Event(), threading.Event()
        responses = []
        request = {"op": "search", "query": "q", "n": 1}

        def encode(query):
            entered.set()
            release.wait(3)
            self.assertFalse(tier.model.closed)
            return np.array([1.0, 0.0])

        tier.model.encode_query = encode
        query = threading.Thread(target=lambda: responses.append(daemon.handle(request)))
        query.start()
        disposer = threading.Thread(target=daemon._dispose, args=(tier,))
        try:
            self.assertTrue(entered.wait(1))
            daemon._withdraw_gpu()
            disposer.start()
            self.assertFalse(tier.model.closed)
            self.assertEqual(daemon.handle(request)["tier"], "cpu")
        finally:
            release.set()
            query.join(3)
            if disposer.ident:
                disposer.join(3)
        self.assertTrue(tier.model.closed)
        self.assertEqual(responses[0]["tier"], "gpu")

    def test_model_libraries_are_not_imported(self):
        self.assertNotIn("torch", sys.modules)
        self.assertNotIn("transformers", sys.modules)
        self.assertNotIn("sentence_transformers", sys.modules)

    def test_warmup_is_error_not_a_false_empty_answer(self):
        daemon = Daemon("/tmp", factory=FakeModel, no_gpu=True)
        with self.assertRaisesRegex(RuntimeError, "warming up"):
            daemon.handle({"op": "related", "prompt": "q", "n": 1})
        self.assertEqual(daemon.handle({"op": "status"})["chunks"], 0)


if __name__ == "__main__":
    unittest.main()
