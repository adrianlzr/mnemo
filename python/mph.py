import random
import uuid
from collections.abc import Iterator
from itertools import islice
from pathlib import Path
from typing import Self

import xxhash

MASK64 = (1 << 64) - 1


def perfect_hash(h: int, a: int, b: int) -> int:
    """Map a 64-bit integer h to a slot in range(2**b)."""
    return ((a * h) & MASK64) >> (64 - b)


def build_perfect_hash_params(
    hashes: list[int], tries_per_size=100
) -> tuple[int, int, int]:
    """Find parameters (a, b, size) so that perfect_hash() has no collisions.

    `hashes` must be distinct 64-bit integers. Starts with the smallest table
    (2**b >= len(hashes)) and grows it if no working multiplier is found. This
    always terminates: at b = 64, any odd multiplier is invertible mod 2**64,
    so distinct inputs give distinct slots.
    """
    if len(set(hashes)) != len(hashes):
        raise ValueError("duplicate hashes: two keys hash to the same 64-bit value")
    b = max(1, (len(hashes) - 1).bit_length())
    rng = random.Random(1)
    while True:
        for _ in range(tries_per_size):
            a = rng.getrandbits(64) | 1  # odd multiplier
            slots = {perfect_hash(h, a, b) for h in hashes}
            if len(slots) == len(hashes):
                return a, b, 2**b
        b += 1


BUNDLES = [
    "com.test.1",
    "com.abc.com",
    "net.abc.123",
    "id12345",
    "id11223344",
]


class Key:
    def __init__(self, bundle: str, idfa: str) -> None:
        self.bundle = bundle
        self.idfa = idfa

    def __str__(self) -> str:
        return f"{self.bundle}{self.idfa}"

    def __repr__(self) -> str:
        return f"Key(bundle={self.bundle!r},idfa={self.idfa!r})"

    @property
    def hash64(self) -> int:
        """Stable 64-bit hash. Use this instead of hash(), which truncates it."""
        return xxhash.xxh3_64_intdigest(str(self).encode())

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Key) and self.value == other.value


class KeyBuilder:
    """Generates `size` distinct random keys (random bundle + random IDFA)."""

    def __init__(self, size: int) -> None:
        self.size = size

    def __iter__(self) -> Iterator[Key]:
        rng = random.Random(self.size)
        for _ in range(self.size):
            bundle = rng.choice(BUNDLES)
            idfa = str(uuid.UUID(int=rng.getrandbits(128), version=4))
            yield Key(bundle, idfa)


class ValueBuilder:
    def __init__(self, size: int) -> None:
        self.size = size

    def __iter__(self) -> Iterator[str]:
        for _ in range(self.size):
            yield str(uuid.uuid4()).upper()


BATCH_SIZE = 100_000


class PerfectHashTableBuilder:
    def __init__(
        self,
        params_path: str,
        output_path: str,
        size: int,
        batch_size: int = BATCH_SIZE,
    ) -> None:
        self.params_path = Path(params_path)
        self.output_path = Path(output_path)
        self.size = size
        self.batch_size = batch_size

    def build(self) -> None:
        hashes = [k.hash64 for k in KeyBuilder(self.size)]
        a, b, size = build_perfect_hash_params(hashes)

        with self.params_path.open("w") as f:
            f.write(f"{a},{b},{size}")

        values = iter(ValueBuilder(self.size))
        # Open once in "w" mode so a previous run's data is overwritten.
        with self.output_path.open("w") as f:
            for s in range(0, len(hashes), self.batch_size):
                f.writelines(
                    f"{perfect_hash(h, a, b)},{h},{next(values)}\n"
                    for h in hashes[s : s + self.batch_size]
                )


class PerfectHashTableParamsLoader:
    def __init__(self, params_path: str) -> None:
        self.params_path = Path(params_path)

    def load(self) -> tuple[int, int, int]:  # a, b, size
        with self.params_path.open("r") as f:
            a, b, size = (int(x) for x in f.readline().split(","))
        return a, b, size


class PerfectHashTableOutputLoader:
    def __init__(
        self, output_size: int, output_path: str, batch_size: int = BATCH_SIZE
    ) -> None:
        self.output_size = output_size
        self.output_path = Path(output_path)
        self.batch_size = batch_size

    def load(self) -> tuple[list[int | None], list[str | None]]:
        hashes: list[int | None] = [None] * self.output_size
        values: list[str | None] = [None] * self.output_size
        with self.output_path.open("r") as f:
            while batch := list(islice(f, self.batch_size)):
                for line in batch:
                    idx, h, value = line.rstrip("\n").split(",", 2)
                    hashes[int(idx)] = int(h)
                    values[int(idx)] = value
        return hashes, values


class PerfectHashTable:
    _PHF_NAME = "params.mph"
    _OUT_NAME = "table.csv"

    @classmethod
    def build(cls, size: int) -> None:
        PerfectHashTableBuilder(
            params_path=cls._PHF_NAME,
            output_path=cls._OUT_NAME,
            size=size,
        ).build()

    @classmethod
    def load(cls) -> Self:
        a, b, size = PerfectHashTableParamsLoader(cls._PHF_NAME).load()
        hashes, values = PerfectHashTableOutputLoader(size, cls._OUT_NAME).load()
        return cls(a, b, hashes, values)

    @classmethod
    def new(cls, size: int) -> Self:
        cls.build(size)
        return cls.load()

    def __init__(
        self, a: int, b: int, hashes: list[int | None], values: list[str | None]
    ) -> None:
        self.a = a
        self.b = b
        self.hashes = hashes
        self.values = values

    def get(self, key: Key) -> str | None:
        """Return the value for key, or None if the key isn't in the table."""
        h = key.hash64
        idx = perfect_hash(h, self.a, self.b)
        if self.hashes[idx] != h:
            return None
        return self.values[idx]


if __name__ == "__main__":
    size = 10000
    table = PerfectHashTable.new(size)
    keys = list(KeyBuilder(size))  # same seed -> same keys as the build
    found = sum(table.get(k) is not None for k in keys)
    missing = Key("com.not.there", str(uuid.uuid4()))
    print(
        f"{found}/{size} keys found, table size {len(table.values)}, "
        f"unknown key -> {table.get(missing)}"
    )
