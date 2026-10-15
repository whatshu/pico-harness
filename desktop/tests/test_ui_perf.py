"""Checks for performance measurement units, sampling windows, and fixture validity."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from ui_perf import proc_cpu_ticks, summarize, validate_fixture


class MeasurementTests(unittest.TestCase):
    def test_proc_comm_can_contain_spaces_and_parentheses(self):
        fields = ["S"] + ["0"] * 10 + ["120", "30"] + ["0"] * 10
        self.assertEqual(proc_cpu_ticks("321 (test app (worker)) " + " ".join(fields)), 150)

    def test_cpu_is_time_weighted_and_warmup_is_excluded(self):
        # Unequal observation intervals: 0.1 CPU seconds over 0.2 wall seconds,
        # followed by 0.8 over 0.8; arithmetic averaging would give the wrong answer.
        samples = [{"time": t, "cpu_s": cpu, "rss_mib": rss, "pss_mib": rss / 2}
                   for t, cpu, rss in [(0, 50, 500), (3, 60, 100), (3.2, 60.1, 102),
                                       (4, 60.9, 104), (5, 61.9, 106), (6, 62.9, 108)]]
        result = summarize(samples, 3, 3)
        self.assertAlmostEqual(result["cpu_mean_pct_one_core"], 100 * 2.9 / 3)
        self.assertEqual(result["rss_peak_mib"], 108)
        self.assertEqual(result["rss_startup_peak_mib"], 500)
        self.assertEqual(result["rss_growth_mib"], 8)
        self.assertGreater(result["rss_slope_mib_per_s"], 0)

    def test_insufficient_samples_do_not_produce_misleading_metrics(self):
        with self.assertRaisesRegex(RuntimeError, "Insufficient"):
            summarize([], 3, 5)

    def test_fixture_failure_and_buffer_overflow_fail_the_run(self):
        self.assertTrue(validate_fixture("connected-live", {"frames": 100, "buffers": {"live_ready": False}}))
        self.assertTrue(validate_fixture("console", {"frames": 100, "buffers": {"console_rows": 5001}}))
        self.assertTrue(validate_fixture("history-limit", {"frames": 100, "buffers": {"history_samples": 199999}}))
        self.assertFalse(validate_fixture("console-flood", {"frames": 100, "buffers": {"console_rows": 5000, "received_bytes": 16 * 1024 * 1024}}))


if __name__ == "__main__":
    unittest.main()
