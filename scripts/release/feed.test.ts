import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { describe, it } from "node:test";
import {
  assetUrl,
  buildFeed,
  checksums,
  releaseAssets,
  updaterAssets,
  verifyFeed,
  type Feed,
} from "./feed.ts";

const REPO = "MoliDuo/MoliWhisper";
const VERSION = "2.1.0";

function signer() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x as string, "base64url");
  const keyId = Buffer.alloc(8, 7);
  const pub = Buffer.from(
    `untrusted comment: minisign public key\n${Buffer.concat([Buffer.from("Ed"), keyId, raw]).toString("base64")}\n`
  ).toString("base64");
  const signFile = (data: Buffer, version: string | null = VERSION) => {
    const file = sign(null, createHash("blake2b512").update(data).digest(), privateKey);
    const comment =
      version === null ? "timestamp:1\tfile:x" : `timestamp:1\tfile:x\tversion:${version}`;
    const global = sign(null, Buffer.concat([file, Buffer.from(comment)]), privateKey);
    return Buffer.from(
      `untrusted comment: sig\n${Buffer.concat([Buffer.from("ED"), keyId, file]).toString("base64")}\ntrusted comment: ${comment}\n${global.toString("base64")}\n`
    ).toString("base64");
  };
  return { pub, signFile };
}

const archive = Buffer.from("mac archive");

function release(options: { signedVersion?: string | null } = {}) {
  const { pub, signFile } = signer();
  const feed = buildFeed({
    repo: REPO,
    version: VERSION,
    notes: "notes",
    pubDate: new Date("2026-10-09T12:00:00Z"),
    signatures: { "darwin-aarch64": signFile(archive, options.signedVersion) },
  });
  const download = async (url: string) => {
    const name = decodeURIComponent(url.split("/").pop() ?? "");
    if (name === updaterAssets(VERSION)["darwin-aarch64"]) return archive;
    throw new Error("404");
  };
  return { pub, feed, download };
}

describe("names", () => {
  it("follow standard 006 6.6.1 and carry the version", () => {
    assert.deepEqual(releaseAssets("2.1.0"), [
      "MoliWhisper_2.1.0_macos_arm64.app.tar.gz",
      "MoliWhisper_2.1.0_macos_arm64.dmg",
    ]);
    for (const name of releaseAssets("2.1.0")) {
      assert.match(name, /^[A-Za-z0-9]+_\d+\.\d+\.\d+_(macos|windows)_(arm64|x64)\.[A-Za-z0-9.]+$/);
    }
  });
});

describe("buildFeed", () => {
  it("points at the files of this version, never at latest, and carries the signatures", () => {
    const { feed } = release();
    assert.equal(feed.version, "2.1.0");
    assert.equal(feed.pub_date, "2026-10-09T12:00:00.000Z");
    const entry = feed.platforms["darwin-aarch64"];
    assert.equal(
      entry?.url,
      "https://github.com/MoliDuo/MoliWhisper/releases/download/v2.1.0/MoliWhisper_2.1.0_macos_arm64.app.tar.gz"
    );
    assert.doesNotMatch(entry?.signature ?? "", /\s/);
    assert.ok(assetUrl(REPO, "1.0.0", "a b.dmg").includes("a%20b.dmg"));
  });
});

describe("checksums", () => {
  it("writes sha256sum's format", () => {
    const text = checksums([
      { name: "a.txt", data: Buffer.from("hello") },
      { name: "b.txt", data: Buffer.from("") },
    ]);
    assert.equal(
      text,
      "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  a.txt\n" +
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  b.txt\n"
    );
  });
});

describe("verifyFeed", () => {
  const check = (feed: unknown, pub: string, download: (url: string) => Promise<Uint8Array>) =>
    verifyFeed({ feed, repo: REPO, version: VERSION, publicKey: pub, download });

  it("passes a feed whose files verify with the shipped key", async () => {
    const { pub, feed, download } = release();
    assert.deepEqual(await check(feed, pub, download), []);
  });

  it("accepts signatures that record no version, as the Tauri CLI makes them today", async () => {
    const { pub, feed, download } = release({ signedVersion: null });
    assert.deepEqual(await check(feed, pub, download), []);
  });

  it("reports a wrong version, a wrong address, a missing platform and unreachable files", async () => {
    const { pub, feed, download } = release();
    const wrongUrl: Feed = {
      ...feed,
      version: "1.9.0",
      platforms: {
        "darwin-aarch64": {
          ...feed.platforms["darwin-aarch64"]!,
          url: "https://example.com/latest",
        },
      },
    };
    const problems = (await check(wrongUrl, pub, download)).join("\n");
    assert.match(problems, /announces 1\.9\.0/);
    assert.match(problems, /points at https:\/\/example\.com\/latest/);
    const missing = (await check({ ...feed, platforms: {} }, pub, download)).join("\n");
    assert.match(missing, /darwin-aarch64: missing/);
    const gone = await check(feed, pub, async () => {
      throw new Error("404");
    });
    assert.match(gone.join("\n"), /cannot download/);
    assert.deepEqual(await check(null, pub, download), ["the feed is not a JSON object"]);
  });

  it("reports a file that does not match its signature, a different key, and a signature for another version", async () => {
    const { pub, feed } = release();
    const tampered = await check(feed, pub, async () => Buffer.from("tampered"));
    assert.match(tampered.join("\n"), /does not match the file/);
    const wrongKey = await check(feed, release().pub, release().download);
    assert.match(wrongKey.join("\n"), /different key|does not match/);
    const old = release({ signedVersion: "1.0.0" });
    const stale = await check(old.feed, old.pub, old.download);
    assert.match(stale.join("\n"), /made for version 1\.0\.0, not 2\.1\.0/);
  });
});
