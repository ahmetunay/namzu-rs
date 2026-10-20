import copy
import importlib.util
from pathlib import Path
import random

spec = importlib.util.spec_from_file_location("candidate", Path.cwd() / "solution.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

def reference(rows):
    first = {}
    for identifier, account, amount in rows:
        first.setdefault(identifier, (account, amount))
    accounts = sorted({account for account, _ in first.values()})
    return {account: sum(amount for name, amount in first.values() if name == account)
            for account in accounts}

cases = [[], [("x", "z", 7), ("x", "a", 99), ("y", "z", -7)],
         [("1", "b", 2), ("2", "a", -4), ("3", "b", 1)]]
rng = random.Random(7303)
for _ in range(80):
    cases.append([(str(rng.randrange(12)), rng.choice(["a", "b", "z", ""]),
                   rng.randrange(-100, 101)) for _ in range(rng.randrange(50))])
for rows in cases:
    before = copy.deepcopy(rows)
    result = module.ledger(rows)
    assert type(result) is dict
    assert result == reference(rows)
    assert list(result) == sorted(result)
    assert rows == before
for row in [("x", "a", True), ("x", "a", 1.0), (1, "a", 2), ("x", 1, 2)]:
    for rows in [[row], [("x", "a", 0), row]]:
        try:
            module.ledger(rows)
        except TypeError:
            pass
        else:
            raise AssertionError("every row must be validated, including duplicates")
