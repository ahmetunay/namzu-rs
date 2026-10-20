import copy
import importlib.util
from pathlib import Path
import sys

# -I keeps ambient packages out; only these two explicit task modules are loaded.
def load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path.cwd() / file)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module

legacy = load("legacy", "legacy.py")
candidate = load("candidate", "solution.py")
for texts in [[], [" A  B\tC ", "", "İSTANBUL", "x", "x"], ["\n\t"]]:
    before = copy.deepcopy(texts)
    assert candidate.normalize_many(texts) == [legacy.normalize(text) for text in texts]
    assert texts == before
assert candidate.normalize_many(iter([" A ", " B "])) == ["a", "b"]
try:
    candidate.normalize_many([1])
except TypeError:
    pass
else:
    raise AssertionError("legacy TypeError must survive")

# Prove actual delegation, not merely equivalent copied code.
calls = []
def probe(text):
    calls.append(text)
    return "probe:" + text
legacy.normalize = probe
# Reload after installing the probe: a direct function import is valid delegation too.
candidate = load("candidate_probe", "solution.py")
assert candidate.normalize_many([" A ", "b", "b"]) == ["probe: A ", "probe:b", "probe:b"]
assert calls == [" A ", "b", "b"]
