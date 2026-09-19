"""
Subscription topic rules.

A subscription topic is either a literal message type or a prefix selector
ending in a single ``*``. ``tick.*`` selects every message type that starts
with ``tick.``, and ``*`` alone selects every message type the engine
publishes, including types that appear later in the run. The wildcard is
terminal: a ``*`` anywhere else could never match, so it is reported as an
error instead of being accepted and ignored.

These are the semantics of the engine's IPC prefix subscriptions. Engine
0.10.10 and earlier accepted a wildcard topic and delivered nothing.
"""

from __future__ import annotations

import re
from typing import Literal

from .errors import ValidationError

# Maximum length, in UTF-8 bytes, of a wildcard subscription topic.
MAX_PATTERN_LEN = 256

# What the engine will do with one requested topic.
TopicKind = Literal["exact", "pattern"]

# A message type the engine can publish: dot-separated, lowercase.
_MESSAGE_TYPE = re.compile(r"^[a-z0-9_-]+(\.[a-z0-9_-]+)*$")


def pattern_prefix(topic: str) -> str | None:
    """
    Return the literal prefix of a terminal-wildcard topic.

    ``tick.*`` yields ``"tick."`` and ``*`` yields ``""``. A topic with no
    trailing ``*`` yields ``None``, and so does one whose prefix contains a
    ``*``, because that topic is not a usable selector.
    """
    if not topic.endswith("*"):
        return None
    prefix = topic[:-1]
    if "*" in prefix:
        return None
    return prefix


def classify_topic(topic: str) -> TopicKind:
    """
    Decide whether a topic is a literal message type or a wildcard selector.

    Args:
        topic: The requested subscription topic

    Returns:
        ``"exact"`` or ``"pattern"``

    Raises:
        ValidationError: If the topic is empty, if a ``*`` appears anywhere but
            the final position, or if a wildcard topic exceeds
            ``MAX_PATTERN_LEN`` bytes. Each of those can never deliver a
            message, so the caller is told rather than left waiting.
    """
    if not topic:
        raise ValidationError("subscription topic cannot be empty", "topic")
    if "*" not in topic:
        return "exact"
    if pattern_prefix(topic) is None:
        usable = topic.split("*", 1)[0]
        raise ValidationError(
            f"subscription topic '{topic}' can never match: '*' is only a wildcard "
            f"as the final character, so write a prefix selector such as "
            f"'{usable}*' instead",
            "topic",
        )
    length = len(topic.encode("utf-8"))
    if length > MAX_PATTERN_LEN:
        raise ValidationError(
            f"subscription topic '{topic}' is {length} bytes, over the "
            f"{MAX_PATTERN_LEN}-byte limit for a wildcard topic",
            "topic",
        )
    return "pattern"


def topic_matches(topic: str, message_type: str) -> bool:
    """
    Test a message type against one subscription topic.

    A literal topic matches only itself. A terminal-wildcard topic matches
    every message type that starts with the text before the ``*``, so ``*``
    matches all of them. A topic with a misplaced wildcard matches nothing.
    """
    prefix = pattern_prefix(topic)
    if prefix is None:
        return "*" not in topic and topic == message_type
    return message_type.startswith(prefix)


def partition_topics(topics: list[str]) -> tuple[list[str], list[str]]:
    """
    Split requested topics into literal names and wildcard selectors.

    Order within each group is the caller's order, and duplicates are kept: the
    engine deduplicates recipients, so a connection subscribed to both
    ``tick.out`` and ``tick.*`` still receives one copy of ``tick.out``.

    Args:
        topics: The requested subscription topics

    Returns:
        A ``(exact, patterns)`` pair

    Raises:
        ValidationError: On the first topic that can never match, before any
            subscription request is sent.
    """
    exact: list[str] = []
    patterns: list[str] = []
    for topic in topics:
        if classify_topic(topic) == "exact":
            exact.append(topic)
        else:
            patterns.append(topic)
    return exact, patterns


def is_emergent_message_type(message_type: str) -> bool:
    """
    Report whether a push notification names an Emergent message type.

    A ``*`` subscription matches every IPC broadcast the engine makes, which
    includes the transport's own envelope names such as ``SystemEvent``. Those
    are the containers Emergent messages travel in, not messages in their own
    right, and the engine forwards what they carry separately under its own
    type. They are not valid Emergent message types, which is how they are
    told apart.
    """
    return bool(_MESSAGE_TYPE.match(message_type))
