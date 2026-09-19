"""Tests for subscription topic rules."""

from __future__ import annotations

import pytest

from emergent.errors import ValidationError
from emergent.topics import (
    MAX_PATTERN_LEN,
    classify_topic,
    is_emergent_message_type,
    partition_topics,
    pattern_prefix,
    topic_matches,
)


def test_a_topic_without_a_star_is_exact() -> None:
    assert classify_topic("tick.out") == "exact"
    assert classify_topic("system.shutdown") == "exact"


def test_a_terminal_star_is_a_pattern() -> None:
    assert classify_topic("tick.*") == "pattern"
    assert classify_topic("system.started.*") == "pattern"
    assert classify_topic("*") == "pattern"
    assert classify_topic("tick*") == "pattern"


@pytest.mark.parametrize("topic", ["*.out", "tick.*.out", "tick.**"])
def test_a_star_anywhere_else_is_rejected(topic: str) -> None:
    with pytest.raises(ValidationError) as excinfo:
        classify_topic(topic)
    assert topic in str(excinfo.value)


def test_the_rejection_message_names_a_usable_selector() -> None:
    with pytest.raises(ValidationError) as excinfo:
        classify_topic("system.*.error")
    assert "'system.*'" in str(excinfo.value)


def test_an_empty_topic_is_rejected() -> None:
    with pytest.raises(ValidationError):
        classify_topic("")


def test_an_oversized_pattern_is_rejected() -> None:
    with pytest.raises(ValidationError) as excinfo:
        classify_topic("a" * MAX_PATTERN_LEN + "*")
    assert str(MAX_PATTERN_LEN) in str(excinfo.value)


def test_a_pattern_exactly_at_the_limit_is_accepted() -> None:
    assert classify_topic("a" * (MAX_PATTERN_LEN - 1) + "*") == "pattern"


def test_pattern_prefix_strips_only_a_terminal_star() -> None:
    assert pattern_prefix("tick.*") == "tick."
    assert pattern_prefix("*") == ""
    assert pattern_prefix("tick.out") is None
    assert pattern_prefix("*.out") is None
    assert pattern_prefix("a*b*") is None


def test_an_exact_topic_matches_only_itself() -> None:
    assert topic_matches("tick.out", "tick.out")
    assert not topic_matches("tick.out", "tick.exit")
    assert not topic_matches("tick.out", "tick.out.deep")


def test_a_pattern_matches_every_message_type_with_its_prefix() -> None:
    assert topic_matches("tick.*", "tick.out")
    assert topic_matches("tick.*", "tick.exit")
    assert topic_matches("tick.*", "tick.out.deep")
    assert not topic_matches("tick.*", "tick")
    assert not topic_matches("tick.*", "ticker.out")
    assert topic_matches("system.started.*", "system.started.ticker")
    assert not topic_matches("system.started.*", "system.stopped.ticker")


def test_a_bare_star_matches_everything() -> None:
    assert topic_matches("*", "tick.out")
    assert topic_matches("*", "system.shutdown")


def test_a_misplaced_wildcard_matches_nothing() -> None:
    assert not topic_matches("*.out", "tick.out")
    assert not topic_matches("*.out", "*.out")


def test_partitioning_keeps_order_and_separates_the_two_kinds() -> None:
    exact, patterns = partition_topics(["tick.out", "tick.*", "system.started.*", "a"])
    assert exact == ["tick.out", "a"]
    assert patterns == ["tick.*", "system.started.*"]


def test_partitioning_reports_the_first_unusable_topic() -> None:
    with pytest.raises(ValidationError) as excinfo:
        partition_topics(["tick.out", "a*b", "c*d"])
    assert "a*b" in str(excinfo.value)


def test_partitioning_keeps_an_overlapping_pair() -> None:
    exact, patterns = partition_topics(["tick.out", "tick.*"])
    assert exact == ["tick.out"]
    assert patterns == ["tick.*"]


def test_emergent_message_types_are_recognised() -> None:
    assert is_emergent_message_type("tick.out")
    assert is_emergent_message_type("system.started.ticker")
    assert is_emergent_message_type("with-hyphen_and_underscore")


def test_transport_envelope_names_are_not_emergent_messages() -> None:
    assert not is_emergent_message_type("SystemEvent")
    assert not is_emergent_message_type("EmergentMessage")
    assert not is_emergent_message_type("")
    assert not is_emergent_message_type("tick..out")
    assert not is_emergent_message_type(".tick")
