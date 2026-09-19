/**
 * Tests for subscription topic rules.
 * @module
 */

import { assert, assertEquals, assertThrows } from "@std/assert";
import { ValidationError } from "./errors.ts";
import {
  classifyTopic,
  isEmergentMessageType,
  MAX_PATTERN_LEN,
  partitionTopics,
  patternPrefix,
  topicMatches,
} from "./topics.ts";

Deno.test("a topic without a star is exact", () => {
  assertEquals(classifyTopic("tick.out"), "exact");
  assertEquals(classifyTopic("system.shutdown"), "exact");
});

Deno.test("a terminal star is a pattern", () => {
  assertEquals(classifyTopic("tick.*"), "pattern");
  assertEquals(classifyTopic("system.started.*"), "pattern");
  assertEquals(classifyTopic("*"), "pattern");
  assertEquals(classifyTopic("tick*"), "pattern");
});

Deno.test("a star anywhere else is rejected", () => {
  for (const topic of ["*.out", "tick.*.out", "tick.**"]) {
    assertThrows(() => classifyTopic(topic), ValidationError, topic);
  }
});

Deno.test("the rejection message names a usable selector", () => {
  assertThrows(
    () => classifyTopic("system.*.error"),
    ValidationError,
    "'system.*'",
  );
});

Deno.test("an empty topic is rejected", () => {
  assertThrows(() => classifyTopic(""), ValidationError);
});

Deno.test("an oversized pattern is rejected", () => {
  assertThrows(
    () => classifyTopic("a".repeat(MAX_PATTERN_LEN) + "*"),
    ValidationError,
    String(MAX_PATTERN_LEN),
  );
});

Deno.test("a pattern exactly at the limit is accepted", () => {
  assertEquals(classifyTopic("a".repeat(MAX_PATTERN_LEN - 1) + "*"), "pattern");
});

Deno.test("patternPrefix strips only a terminal star", () => {
  assertEquals(patternPrefix("tick.*"), "tick.");
  assertEquals(patternPrefix("*"), "");
  assertEquals(patternPrefix("tick.out"), null);
  assertEquals(patternPrefix("*.out"), null);
  assertEquals(patternPrefix("a*b*"), null);
});

Deno.test("an exact topic matches only itself", () => {
  assert(topicMatches("tick.out", "tick.out"));
  assert(!topicMatches("tick.out", "tick.exit"));
  assert(!topicMatches("tick.out", "tick.out.deep"));
});

Deno.test("a pattern matches every message type with its prefix", () => {
  assert(topicMatches("tick.*", "tick.out"));
  assert(topicMatches("tick.*", "tick.exit"));
  assert(topicMatches("tick.*", "tick.out.deep"));
  assert(!topicMatches("tick.*", "tick"));
  assert(!topicMatches("tick.*", "ticker.out"));
  assert(topicMatches("system.started.*", "system.started.ticker"));
  assert(!topicMatches("system.started.*", "system.stopped.ticker"));
});

Deno.test("a bare star matches everything", () => {
  assert(topicMatches("*", "tick.out"));
  assert(topicMatches("*", "system.shutdown"));
});

Deno.test("a misplaced wildcard matches nothing", () => {
  assert(!topicMatches("*.out", "tick.out"));
  assert(!topicMatches("*.out", "*.out"));
});

Deno.test("partitioning keeps order and separates the two kinds", () => {
  const { exact, patterns } = partitionTopics([
    "tick.out",
    "tick.*",
    "system.started.*",
    "a",
  ]);
  assertEquals(exact, ["tick.out", "a"]);
  assertEquals(patterns, ["tick.*", "system.started.*"]);
});

Deno.test("partitioning reports the first unusable topic", () => {
  assertThrows(
    () => partitionTopics(["tick.out", "a*b", "c*d"]),
    ValidationError,
    "a*b",
  );
});

Deno.test("partitioning keeps an overlapping pair", () => {
  const { exact, patterns } = partitionTopics(["tick.out", "tick.*"]);
  assertEquals(exact, ["tick.out"]);
  assertEquals(patterns, ["tick.*"]);
});

Deno.test("Emergent message types are recognised", () => {
  assert(isEmergentMessageType("tick.out"));
  assert(isEmergentMessageType("system.started.ticker"));
  assert(isEmergentMessageType("with-hyphen_and_underscore"));
});

Deno.test("transport envelope names are not Emergent messages", () => {
  assert(!isEmergentMessageType("SystemEvent"));
  assert(!isEmergentMessageType("EmergentMessage"));
  assert(!isEmergentMessageType(""));
  assert(!isEmergentMessageType("tick..out"));
  assert(!isEmergentMessageType(".tick"));
});
