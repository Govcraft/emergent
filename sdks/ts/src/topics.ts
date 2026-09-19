/**
 * Subscription topic rules.
 *
 * A subscription topic is either a literal message type or a prefix selector
 * ending in a single `*`. `tick.*` selects every message type that starts with
 * `tick.`, and `*` alone selects every message type the engine publishes,
 * including types that appear later in the run. The wildcard is terminal: a
 * `*` anywhere else could never match, so it is reported as an error instead
 * of being accepted and ignored.
 *
 * These are the semantics of the engine's IPC prefix subscriptions. Engine
 * 0.10.10 and earlier accepted a wildcard topic and delivered nothing.
 *
 * @module
 */

import { ValidationError } from "./errors.ts";

/** Maximum length, in UTF-8 bytes, of a wildcard subscription topic. */
export const MAX_PATTERN_LEN = 256;

/** What the engine will do with one requested topic. */
export type TopicKind = "exact" | "pattern";

/** Requested topics split into the two subscription kinds the engine accepts. */
export interface TopicPartition {
  /** Literal message types, subscribed by name. */
  exact: string[];
  /** Terminal-wildcard selectors, subscribed as IPC patterns. */
  patterns: string[];
}

/** A message type the engine can publish: dot-separated, lowercase. */
const MESSAGE_TYPE = /^[a-z0-9_-]+(\.[a-z0-9_-]+)*$/;

const utf8 = new TextEncoder();

/**
 * Return the literal prefix of a terminal-wildcard topic.
 *
 * `tick.*` yields `"tick."` and `*` yields `""`. A topic with no trailing `*`
 * yields `null`, and so does one whose prefix contains a `*`, because that
 * topic is not a usable selector.
 */
export function patternPrefix(topic: string): string | null {
  if (!topic.endsWith("*")) return null;
  const prefix = topic.slice(0, -1);
  if (prefix.includes("*")) return null;
  return prefix;
}

/**
 * Decide whether a topic is a literal message type or a wildcard selector.
 *
 * @throws {ValidationError} If the topic is empty, if a `*` appears anywhere
 * but the final position, or if a wildcard topic exceeds
 * {@link MAX_PATTERN_LEN} bytes. Each of those can never deliver a message, so
 * the caller is told rather than left waiting.
 */
export function classifyTopic(topic: string): TopicKind {
  if (topic.length === 0) {
    throw new ValidationError("subscription topic cannot be empty", "topic");
  }
  if (!topic.includes("*")) return "exact";
  if (patternPrefix(topic) === null) {
    const usable = topic.slice(0, topic.indexOf("*"));
    throw new ValidationError(
      `subscription topic '${topic}' can never match: '*' is only a wildcard ` +
        `as the final character, so write a prefix selector such as ` +
        `'${usable}*' instead`,
      "topic",
    );
  }
  const length = utf8.encode(topic).length;
  if (length > MAX_PATTERN_LEN) {
    throw new ValidationError(
      `subscription topic '${topic}' is ${length} bytes, over the ` +
        `${MAX_PATTERN_LEN}-byte limit for a wildcard topic`,
      "topic",
    );
  }
  return "pattern";
}

/**
 * Test a message type against one subscription topic.
 *
 * A literal topic matches only itself. A terminal-wildcard topic matches every
 * message type that starts with the text before the `*`, so `*` matches all of
 * them. A topic with a misplaced wildcard matches nothing.
 */
export function topicMatches(topic: string, messageType: string): boolean {
  const prefix = patternPrefix(topic);
  if (prefix === null) return !topic.includes("*") && topic === messageType;
  return messageType.startsWith(prefix);
}

/**
 * Split requested topics into literal names and wildcard selectors.
 *
 * Order within each group is the caller's order, and duplicates are kept: the
 * engine deduplicates recipients, so a connection subscribed to both
 * `tick.out` and `tick.*` still receives one copy of `tick.out`.
 *
 * @throws {ValidationError} On the first topic that can never match, before
 * any subscription request is sent.
 */
export function partitionTopics(topics: string[]): TopicPartition {
  const exact: string[] = [];
  const patterns: string[] = [];
  for (const topic of topics) {
    if (classifyTopic(topic) === "exact") exact.push(topic);
    else patterns.push(topic);
  }
  return { exact, patterns };
}

/**
 * Report whether a push notification names an Emergent message type.
 *
 * A `*` subscription matches every IPC broadcast the engine makes, which
 * includes the transport's own envelope names such as `SystemEvent`. Those are
 * the containers Emergent messages travel in, not messages in their own right,
 * and the engine forwards what they carry separately under its own type. They
 * are not valid Emergent message types, which is how they are told apart.
 */
export function isEmergentMessageType(messageType: string): boolean {
  return MESSAGE_TYPE.test(messageType);
}
