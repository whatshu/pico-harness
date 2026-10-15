"""Analysis helpers for the Work-core direct-GPIO self-test pattern."""

from dataclasses import dataclass
import math

DIRECT_GPIO_PINS = (*range(12, 23), *range(26, 29))
def decode_direct_pattern(sample):
    return ((sample >> 12) & 0x7FF) | (((sample >> 26) & 0x7) << 11)


def _direct_patterns():
    patterns = []
    state = 0
    for index in range(32):
        patterns.append(state)
        bit = index % 14 if index < 28 else (index - 28) & 1
        state ^= 1 << bit
    return tuple(patterns)


DIRECT_PATTERNS = _direct_patterns()
PATTERN_PHASES = {
    state: tuple(index for index, value in enumerate(DIRECT_PATTERNS) if value == state)
    for state in set(DIRECT_PATTERNS)
}


@dataclass(frozen=True)
class LogicAnalysis:
    transition_count: int
    transition_errors: int
    invalid_samples: int
    maximum_delta: int
    expected_maximum_delta: int
    inactive_pins: tuple[int, ...]

    @property
    def sequence_stable(self):
        return self.transition_errors == 0 and self.invalid_samples == 0


def analyze_direct_pattern(samples, sample_frequency, pattern_frequency=1_000_000):
    if len(samples) < 2:
        raise ValueError("at least two samples are required")
    if sample_frequency <= 0 or pattern_frequency <= 0:
        raise ValueError("frequencies must be positive")

    states = [decode_direct_pattern(sample) for sample in samples]
    phases = [PATTERN_PHASES.get(state, ()) for state in states]
    deltas = []
    previous = ()
    for current in phases:
        if not current:
            previous = ()
            continue
        if previous:
            deltas.append(min(
                (current_phase - previous_phase) % len(DIRECT_PATTERNS)
                for previous_phase in previous for current_phase in current
            ))
        previous = current
    # One extra pattern step accommodates phase quantization between the two
    # independently clocked RP2350s. Larger jumps indicate a missed/torn sample.
    expected_maximum = math.ceil(pattern_frequency / sample_frequency) + 1
    seen_high = 0
    seen_low = 0
    for sample in samples:
        seen_high |= sample
        seen_low |= ~sample
    inactive = tuple(
        pin for pin in DIRECT_GPIO_PINS
        if not (seen_high & (1 << pin)) or not (seen_low & (1 << pin))
    )
    return LogicAnalysis(
        transition_count=len(deltas),
        transition_errors=sum(delta > expected_maximum for delta in deltas),
        invalid_samples=sum(not phase for phase in phases),
        maximum_delta=max(deltas, default=0),
        expected_maximum_delta=expected_maximum,
        inactive_pins=inactive,
    )
