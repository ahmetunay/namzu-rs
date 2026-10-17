import importlib.util
from pathlib import Path
import random
import re
import unicodedata

spec = importlib.util.spec_from_file_location("candidate", Path.cwd() / "solution.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
fixed = {
    "": "",
    "  Hello, World! ": "hello-world",
    "Crème brûlée": "creme-brulee",
    "İstanbul Çığ": "istanbul-cg",
    "a--b__c": "a-b-c",
    "😀": "",
    "A123": "a123",
}
for text, expected in fixed.items():
    assert module.slugify(text) == expected, (text, expected)
rng = random.Random(9042)
alphabet = "aBcD012 _-,.!éüİç😀"
for _ in range(160):
    text = "".join(rng.choice(alphabet) for _ in range(rng.randrange(80)))
    plain = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode().lower()
    expected = re.sub("[^a-z0-9]+", "-", plain).strip("-")
    assert module.slugify(text) == expected
