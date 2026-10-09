import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { checkTag, compareVersions, previousTag } from "./version.ts";

describe("compareVersions", () => {
  it("compares numerically, not as text", () => {
    assert.ok(compareVersions("2.0.0", "1.9.9") > 0);
    assert.ok(compareVersions("1.10.0", "1.9.0") > 0);
    assert.equal(compareVersions("1.0.7", "1.0.7"), 0);
    assert.ok(compareVersions("1.0.6", "1.0.7") < 0);
  });
});

describe("checkTag", () => {
  const tags = ["v1.0.6", "v1.0.7"];
  it("accepts the version file's version when it is newer than every tag", () => {
    assert.deepEqual(checkTag("v2.0.0", "2.0.0", [...tags, "v2.0.0"]), []);
  });
  it("rejects a tag that is not vX.Y.Z or does not match package.json", () => {
    assert.deepEqual(checkTag("2.0.0", "2.0.0", tags), ["the tag 2.0.0 is not vX.Y.Z"]);
    assert.match(checkTag("v2.0.1", "2.0.0", tags)[0] ?? "", /package\.json says 2\.0\.0/);
  });
  it("rejects going backwards or repeating a version", () => {
    assert.match(checkTag("v1.0.5", "1.0.5", tags)[0] ?? "", /not newer than v1\.0\.6, v1\.0\.7/);
    assert.deepEqual(checkTag("v1.0.7", "1.0.7", ["v1.0.7", "v1.0.7"]), []);
    assert.match(checkTag("v1.0.7", "1.0.7", ["v1.0.6", "v1.0.8"])[0] ?? "", /v1\.0\.8/);
  });
});

describe("previousTag", () => {
  it("finds the closest older release tag", () => {
    assert.equal(previousTag("v2.0.0", ["v1.0.6", "v1.0.7", "v2.0.0", "junk"]), "v1.0.7");
    assert.equal(previousTag("v1.0.6", ["v1.0.6"]), null);
  });
});
