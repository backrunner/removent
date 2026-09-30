"""Validate native CPU units against Python's independent process CPU clock."""
import os
import time
import unittest
from benchmark_relay import sampler


class SamplerTests(unittest.TestCase):
    def test_native_cpu_clock_matches_process_time(self):
        read = sampler()
        before = read(os.getpid())[0]
        started = time.process_time()
        while time.process_time() - started < 0.2:
            pass
        elapsed = time.process_time() - started
        after, resident, threads = read(os.getpid())
        self.assertAlmostEqual(after - before, elapsed, delta=0.02)
        self.assertGreater(resident, 0)
        self.assertGreaterEqual(threads, 1)
