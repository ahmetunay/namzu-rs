import copy
import importlib.util
from pathlib import Path
import random

spec = importlib.util.spec_from_file_location("candidate", Path.cwd() / "solution.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
fixed = [([], []), ([(1, 3), (3, 5)], [(1, 5)]), ([(0, 0)], []),
         ([(4, 7), (-3, 1), (0, 6), (8, 8)], [(-3, 7)]),
         ([(1, 10), (2, 3)], [(1, 10)])]
for intervals, expected in fixed:
    before = copy.deepcopy(intervals)
    assert module.merge_intervals(intervals) == expected
    assert intervals == before
for invalid, error in [([(2, 1)], ValueError), ([(1.0, 2)], TypeError),
                       ([(True, 2)], TypeError), ([(0, "2")], TypeError)]:
    try:
        module.merge_intervals(invalid)
    except error:
        pass
    else:
        raise AssertionError("invalid interval accepted")
rng = random.Random(9042)
for _ in range(160):
    intervals = []
    for _ in range(rng.randrange(20)):
        start = rng.randrange(-10, 11)
        intervals.append((start, start + rng.randrange(10)))
    covered = sorted({unit for start, end in intervals for unit in range(start, end)})
    expected = []
    for unit in covered:
        if expected and expected[-1][1] == unit:
            expected[-1] = (expected[-1][0], unit + 1)
        else:
            expected.append((unit, unit + 1))
    before = copy.deepcopy(intervals)
    assert module.merge_intervals(intervals) == expected
    assert intervals == before
