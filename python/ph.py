import random
from dataclasses import dataclass, field
from typing import Any, Self

PRIME = 2**89 - 1


def make_index(a: int, b: int, n: int):
    mask = n - 1

    def index(h: int) -> int:
        # x = a * h + b
        # x = (x & PRIME) + (x >> 89)
        # if x >= PRIME:
        #     x -= PRIME
        # return x & mask
        return ((a * h + b) % PRIME) % n

    return index


@dataclass
class PerfectHash:
    a: int
    b: int
    n: int

    @staticmethod
    def index(h: int) -> int:
        raise NotImplementedError("Must create instances using the build classmethod.")

    @staticmethod
    def build(hashes: list[int], n: int) -> Self:
        assert len(hashes) <= n, "Cannot build perfect hash with n > len(hashes)"

        rng = random.Random()

        max_attempts = 1_000_000
        for attempt in range(max_attempts):
            a = rng.randrange(1, PRIME)
            b = rng.randrange(0, PRIME)
            idx_fn = make_index(a, b, n)

            seen = set()
            collision = False

            for h in hashes:
                idx = idx_fn(h)
                if idx in seen:
                    print(attempt, idx, a, b)
                    collision = True
                    break
                seen.add(idx)

            if not collision:
                ph = PerfectHash(
                    a=a,
                    b=b,
                    n=n,
                )
                ph.index = idx_fn
                return ph

        raise RuntimeError(
            f"Failed to find perfect hash after {max_attempts} attempts."
        )


@dataclass
class PerfectHashTable:
    slots: list[Any] = field(default_factory=list)

    @staticmethod
    def index(h: int) -> int:
        raise NotImplementedError("Must create instances using the build classmethod.")

    @staticmethod
    def build(data: list[tuple[int, Any]]) -> Self:
        ph = PerfectHash.build([h for h, _ in data], n=len(data))
        idx_fn = ph.index

        slots = [None] * ph.n
        for h, v in data:
            idx = idx_fn(h)
            slots[idx] = v

        pht = PerfectHashTable(slots)
        pht.index = idx_fn
        return pht

    def get(self, h: int) -> int:
        idx = self.index(h)
        return self.slots[idx]


import xxhash


def h64(key: int) -> int:
    return xxhash.xxh3_64_intdigest(key.to_bytes(8, "little", signed=True))


keys = range(30)

pairs = [(h64(k), k) for k in keys]

table = PerfectHashTable.build(pairs)
