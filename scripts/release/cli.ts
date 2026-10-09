// Command-line side of the release workflow (.github/workflows/release.yml). The logic is in the other
// files here, with tests; this only reads files, runs git and fetches URLs. Run with `node` (24 runs
// TypeScript directly): `node scripts/release/cli.ts <command> ...`.
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import {
  buildFeed,
  checksums,
  releaseAssets,
  updaterAssets,
  verifyFeed,
  type PlatformKey,
} from "./feed.ts";
import { buildNotes, type Commit } from "./notes.ts";
import { checkTag, previousTag } from "./version.ts";

const git = (...args: string[]) => execFileSync("git", args, { encoding: "utf-8" });
const tags = () => git("tag", "--list", "v*").split("\n").filter(Boolean);
const fail = (messages: string[]): never => {
  for (const message of messages) console.error(`::error::${message}`);
  process.exit(1);
};

const [command, ...args] = process.argv.slice(2);

if (command === "check-tag") {
  // check-tag <tag>: the tag against package.json and the tags that already exist.
  const [tag = ""] = args;
  const { version } = JSON.parse(readFileSync("package.json", "utf-8")) as { version: string };
  const problems = checkTag(tag, version, tags());
  if (problems.length) fail(problems);
  console.log(`${tag} matches package.json (${version})`);
} else if (command === "notes") {
  // notes <tag> <sha>: Release notes for the commits since the previous release.
  const [tag = "", sha = ""] = args;
  const version = tag.replace(/^v/, "");
  const previous = previousTag(tag, tags());
  const range = previous ? `${previous}..${sha}` : sha;
  const log = git("log", "--format=%s%x1f%b%x1e", range);
  const commits: Commit[] = log
    .split("\x1e")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [subject = "", body = ""] = entry.split("\x1f");
      return { subject: subject.trim(), body: body.trim() };
    });
  process.stdout.write(
    buildNotes({
      commits,
      version,
      sha,
      downloads: [["macOS 13+（Apple 芯片）", `MoliWhisper_${version}_macos_arm64.dmg`]],
    })
  );
} else if (command === "assemble") {
  // assemble <dir> <version> <repo> <notes file> <public key>: check what the builds produced, then write
  // latest.json and SHA256SUMS next to it.
  const [dir = "", version = "", repo = "", notesFile = "", publicKey = ""] = args;
  const present = new Set(readdirSync(dir));
  const missing = [
    ...releaseAssets(version),
    ...Object.values(updaterAssets(version)).map((n) => `${n}.sig`),
  ].filter((name) => !present.has(name));
  if (missing.length) fail([`missing build output: ${missing.join(", ")}`]);
  const names = updaterAssets(version);
  const signatures = Object.fromEntries(
    (Object.keys(names) as PlatformKey[]).map((key) => [
      key,
      readFileSync(join(dir, `${names[key]}.sig`), "utf-8"),
    ])
  ) as Record<PlatformKey, string>;
  const feed = buildFeed({
    repo,
    version,
    notes: readFileSync(notesFile, "utf-8"),
    pubDate: new Date(),
    signatures,
  });
  // The same check the published feed gets, against the local files, before anything goes public.
  const problems = await verifyFeed({
    feed,
    repo,
    version,
    publicKey,
    download: async (url) => readFileSync(join(dir, decodeURIComponent(basename(url)))),
  });
  if (problems.length) fail(problems);
  writeFileSync(join(dir, "latest.json"), `${JSON.stringify(feed, null, 2)}\n`);
  const files = releaseAssets(version).map((name) => ({
    name,
    data: readFileSync(join(dir, name)),
  }));
  writeFileSync(join(dir, "SHA256SUMS"), checksums(files));
  console.log(`wrote latest.json and SHA256SUMS for ${version}`);
} else if (command === "verify-published") {
  // verify-published <repo> <version> <public key>: the feed the installed apps will read, as they read it.
  const [repo = "", version = "", publicKey = ""] = args;
  const url = `https://github.com/${repo}/releases/latest/download/latest.json`;
  let last: string[] = [];
  for (let attempt = 1; attempt <= 6; attempt++) {
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      last = await verifyFeed({
        feed: await response.json(),
        repo,
        version,
        publicKey,
        download: async (asset) => {
          const file = await fetch(asset);
          if (!file.ok) throw new Error(`HTTP ${file.status}`);
          return new Uint8Array(await file.arrayBuffer());
        },
      });
    } catch (error) {
      last = [`cannot read ${url}: ${(error as Error).message}`];
    }
    if (last.length === 0) break;
    await new Promise((resolve) => setTimeout(resolve, 10_000));
  }
  if (last.length) fail(last);
  console.log(`the published feed announces ${version} and its files verify`);
} else {
  console.error("usage: cli.ts check-tag | notes | assemble | verify-published");
  process.exit(2);
}
