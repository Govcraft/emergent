"""Tests for the subscription-topic resolution rule used by EmergentSink.messages."""

from __future__ import annotations

from emergent.sink import needs_configured_topics, resolve_topics


def test_requested_topics_win_over_configured() -> None:
    assert resolve_topics(["a.b"], ["c.d", "e.f"]) == ["a.b"]


def test_empty_request_falls_back_to_configured() -> None:
    assert resolve_topics([], ["c.d", "e.f"]) == ["c.d", "e.f"]


def test_empty_request_and_empty_config_resolve_to_nothing() -> None:
    assert resolve_topics([], []) == []


def test_requested_topics_survive_an_empty_config() -> None:
    assert resolve_topics(["a.b", "a.c"], []) == ["a.b", "a.c"]


def test_requested_order_and_duplicates_are_preserved() -> None:
    assert resolve_topics(["b", "a", "b"], ["z"]) == ["b", "a", "b"]


def test_configured_list_is_only_needed_when_nothing_was_requested() -> None:
    assert needs_configured_topics([]) is True
    assert needs_configured_topics(["a.b"]) is False
