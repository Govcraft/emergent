"""The version the package reports is the version the package is released as.

``__version__`` said ``0.1.0`` through SDK 0.13.1 because nothing tied it to
``pyproject.toml``.
"""

import tomllib
from pathlib import Path

import emergent


def test_the_reported_version_is_the_released_version() -> None:
    pyproject = Path(__file__).resolve().parent.parent / "pyproject.toml"
    released = tomllib.loads(pyproject.read_text())["project"]["version"]
    assert emergent.__version__ == released
