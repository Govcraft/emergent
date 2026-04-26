"""Game of Life world handler.

Holds grid state, evolves it on each tick, publishes each generation
for downstream sinks to render. Self-seeds on the first tick using
the LIFE_PATTERN env var, so it doesn't race a one-shot seed source.
"""

import asyncio
import os
from emergent import run_handler, create_message

ROWS = 40
COLS = 80
DEFAULT_PATTERN = os.environ.get("LIFE_PATTERN", "r-pentomino")

# Grid state: list of living cell (row, col) tuples for sparse representation
grid = set()
generation = 0

# Classic patterns
PATTERNS = {
    "glider": [(0, 1), (1, 2), (2, 0), (2, 1), (2, 2)],
    "blinker": [(1, 0), (1, 1), (1, 2)],
    "pulsar": [
        # Quarter pattern, mirrored 4 ways
        (2, 4), (2, 5), (2, 6), (2, 10), (2, 11), (2, 12),
        (4, 2), (4, 7), (4, 9), (4, 14),
        (5, 2), (5, 7), (5, 9), (5, 14),
        (6, 2), (6, 7), (6, 9), (6, 14),
        (7, 4), (7, 5), (7, 6), (7, 10), (7, 11), (7, 12),
        (9, 4), (9, 5), (9, 6), (9, 10), (9, 11), (9, 12),
        (10, 2), (10, 7), (10, 9), (10, 14),
        (11, 2), (11, 7), (11, 9), (11, 14),
        (12, 2), (12, 7), (12, 9), (12, 14),
        (14, 4), (14, 5), (14, 6), (14, 10), (14, 11), (14, 12),
    ],
    "r-pentomino": [(0, 1), (0, 2), (1, 0), (1, 1), (2, 1)],
    "acorn": [(0, 1), (1, 3), (2, 0), (2, 1), (2, 4), (2, 5), (2, 6)],
    "gosper-gun": [
        (5, 1), (5, 2), (6, 1), (6, 2),
        (3, 13), (3, 14), (4, 12), (4, 16), (5, 11), (5, 17),
        (6, 11), (6, 15), (6, 17), (6, 18), (7, 11), (7, 17),
        (8, 12), (8, 16), (9, 13), (9, 14),
        (1, 25), (2, 23), (2, 25), (3, 21), (3, 22),
        (4, 21), (4, 22), (5, 21), (5, 22),
        (6, 23), (6, 25), (7, 25),
        (3, 35), (3, 36), (4, 35), (4, 36),
    ],
}


def seed_pattern(name, offset_r=None, offset_c=None):
    """Place a named pattern on the grid, centered by default."""
    cells = PATTERNS.get(name, PATTERNS["r-pentomino"])
    if offset_r is None:
        max_r = max(r for r, c in cells) if cells else 0
        max_c = max(c for r, c in cells) if cells else 0
        offset_r = (ROWS - max_r) // 2
        offset_c = (COLS - max_c) // 2
    for r, c in cells:
        nr, nc = r + offset_r, c + offset_c
        if 0 <= nr < ROWS and 0 <= nc < COLS:
            grid.add((nr, nc))


def evolve():
    """Apply Game of Life rules, return new grid."""
    neighbor_counts = {}
    for r, c in grid:
        for dr in (-1, 0, 1):
            for dc in (-1, 0, 1):
                if dr == 0 and dc == 0:
                    continue
                nr, nc = (r + dr) % ROWS, (c + dc) % COLS
                neighbor_counts[(nr, nc)] = neighbor_counts.get((nr, nc), 0) + 1

    new_grid = set()
    for cell, count in neighbor_counts.items():
        if count == 3 or (count == 2 and cell in grid):
            new_grid.add(cell)
    return new_grid


def grid_to_payload():
    """Convert grid to a JSON-serializable payload."""
    return {
        "metric": "life",
        "generation": generation,
        "population": len(grid),
        "rows": ROWS,
        "cols": COLS,
        "cells": sorted([r * COLS + c for r, c in grid]),
    }


async def process(msg, handler):
    global grid, generation

    if msg.message_type != "life.tick":
        return

    if not grid and generation == 0:
        seed_pattern(DEFAULT_PATTERN)
    else:
        grid = evolve()
        generation += 1

    await handler.publish(
        create_message("life.frame").caused_by(msg.id).payload(grid_to_payload())
    )


asyncio.run(
    run_handler(
        None,
        ["life.tick"],
        process,
    )
)
