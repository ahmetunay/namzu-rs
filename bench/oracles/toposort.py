import copy
import importlib.util
from itertools import permutations
from pathlib import Path
import random

spec = importlib.util.spec_from_file_location("candidate", Path.cwd() / "solution.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for nodes, edges, expected in [([], [], []), (["c", "b", "a"], [], ["a", "b", "c"]),
                              (["a", "a", "b"], [("a", "b"), ("a", "b")], ["a", "b"]),
                              (["a", "b", "c"], [("c", "a")], ["b", "c", "a"])]:
    before = copy.deepcopy((nodes, edges))
    assert module.plan(nodes, edges) == expected
    assert (nodes, edges) == before
for nodes, edges in [(["a"], [("a", "a")]), (["a", "b"], [("a", "b"), ("b", "a")]),
                     (["a"], [("a", "unknown")])]:
    try:
        module.plan(nodes, edges)
    except ValueError:
        pass
    else:
        raise AssertionError("invalid graph accepted")
rng = random.Random(9042)
for _ in range(40):
    nodes = ["a", "b", "c", "d", "e", "f"]
    order = nodes[:]
    rng.shuffle(order)
    edges = [(order[a], order[b]) for a in range(6) for b in range(a + 1, 6) if rng.randrange(3) == 0]
    expected = next(list(p) for p in permutations(nodes)
                    if all(p.index(a) < p.index(b) for a, b in edges))
    rng.shuffle(nodes)
    before = copy.deepcopy((nodes, edges))
    assert module.plan(nodes, edges) == expected
    assert (nodes, edges) == before
