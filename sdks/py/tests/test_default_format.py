"""The SDK's default wire format, pinned across every public entry point."""

import inspect

import pytest

from emergent import EmergentHandler, EmergentSink, EmergentSource
from emergent._client import BaseClient
from emergent._protocol import DEFAULT_FORMAT, Format, encode_frame


def test_default_format_is_messagepack() -> None:
    assert DEFAULT_FORMAT is Format.MSGPACK


def test_encode_frame_defaults_to_messagepack() -> None:
    frame = encode_frame(1, {"a": 1})
    assert frame[6] == Format.MSGPACK


def test_base_client_defaults_to_messagepack() -> None:
    assert BaseClient.__dataclass_fields__["format"].default is Format.MSGPACK


def _format_defaults(cls: type) -> list[tuple[str, object]]:
    found = []
    for name, member in inspect.getmembers(cls):
        if name.startswith("__") and name != "__init__":
            continue
        try:
            params = inspect.signature(member).parameters
        except (TypeError, ValueError):
            continue
        if "format_" in params:
            found.append((name, params["format_"].default))
    return found


@pytest.mark.parametrize("cls", [EmergentSource, EmergentHandler, EmergentSink])
def test_every_entry_point_defaults_to_messagepack(cls: type) -> None:
    defaults = _format_defaults(cls)
    assert defaults, f"{cls.__name__} exposes no format_ parameter"
    for name, default in defaults:
        assert default is Format.MSGPACK, f"{cls.__name__}.{name} defaults to {default!r}"
