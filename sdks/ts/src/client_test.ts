import { assertEquals } from "@std/assert";
import { parseUnwrapFlag } from "./client.ts";

Deno.test("parseUnwrapFlag enables unwrapping for true and 1", () => {
  for (const value of ["true", "1", "TRUE", "True", " true "]) {
    assertEquals(
      parseUnwrapFlag(value),
      true,
      `value: ${JSON.stringify(value)}`,
    );
  }
});

Deno.test("parseUnwrapFlag leaves unwrapping off for everything else", () => {
  for (const value of [undefined, "", "false", "0", "no", "off", "yes", "2"]) {
    assertEquals(
      parseUnwrapFlag(value),
      false,
      `value: ${JSON.stringify(value)}`,
    );
  }
});
