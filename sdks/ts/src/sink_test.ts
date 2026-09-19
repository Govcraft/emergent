/**
 * Tests for the subscription-topic resolution rule used by EmergentSink.messages.
 * @module
 */

import { assertEquals } from "@std/assert";
import { needsConfiguredTopics, resolveTopics } from "./sink.ts";

Deno.test("requested topics win over configured", () => {
  assertEquals(resolveTopics(["a.b"], ["c.d", "e.f"]), ["a.b"]);
});

Deno.test("empty request falls back to configured", () => {
  assertEquals(resolveTopics([], ["c.d", "e.f"]), ["c.d", "e.f"]);
});

Deno.test("empty request and empty config resolve to nothing", () => {
  assertEquals(resolveTopics([], []), []);
});

Deno.test("requested topics survive an empty config", () => {
  assertEquals(resolveTopics(["a.b", "a.c"], []), ["a.b", "a.c"]);
});

Deno.test("requested order and duplicates are preserved", () => {
  assertEquals(resolveTopics(["b", "a", "b"], ["z"]), ["b", "a", "b"]);
});

Deno.test("configured list is only needed when nothing was requested", () => {
  assertEquals(needsConfiguredTopics([]), true);
  assertEquals(needsConfiguredTopics(["a.b"]), false);
});
