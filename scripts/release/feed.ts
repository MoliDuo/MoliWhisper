// The update feed (`latest.json`), the checksums file and the checks that run on them (standard 006, 6.5
// and 6.6; standard 007, 7.3). All pure: the command-line wrapper does the file and network work.
import { createHash } from "node:crypto";
import { verifyMinisign } from "./minisign.ts";

export const PRODUCT = "MoliWhisper";

/** What the updater downloads per platform, and the asset's name (standard 006, 6.6.1). */
export function updaterAssets(version: string) {
  return {
    "darwin-aarch64": `${PRODUCT}_${version}_macos_arm64.app.tar.gz`,
  } as const;
}

/** Everything a release carries besides the checksums and the feed. */
export function releaseAssets(version: string): string[] {
  return [...Object.values(updaterAssets(version)), `${PRODUCT}_${version}_macos_arm64.dmg`];
}

export type PlatformKey = keyof ReturnType<typeof updaterAssets>;

export interface Feed {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
}

/** The address of one asset of one release: fixed to that version, never `latest` (standard 007, 7.3.1). */
export const assetUrl = (repo: string, version: string, name: string) =>
  `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(name)}`;

export function buildFeed(options: {
  repo: string;
  version: string;
  notes: string;
  pubDate: Date;
  /** The `.sig` file's text for each platform. */
  signatures: Record<PlatformKey, string>;
}): Feed {
  const names = updaterAssets(options.version);
  const platforms: Feed["platforms"] = {};
  for (const key of Object.keys(names) as PlatformKey[]) {
    platforms[key] = {
      signature: options.signatures[key].trim(),
      url: assetUrl(options.repo, options.version, names[key]),
    };
  }
  return {
    version: options.version,
    notes: options.notes,
    pub_date: options.pubDate.toISOString(),
    platforms,
  };
}

/** `sha256sum`'s format, so `sha256sum -c SHA256SUMS` works for the people downloading. */
export function checksums(files: { name: string; data: Uint8Array }[]): string {
  return files
    .map(({ name, data }) => `${createHash("sha256").update(data).digest("hex")}  ${name}\n`)
    .join("");
}

/**
 * Standard 006, 6.5.5 and 007, 7.3.3: what is published must be what the app will accept. Reads the feed
 * as the updater would, downloads each file it points at, and checks the signature with the key that is
 * built into the app. Returns the problems; empty means good.
 */
export async function verifyFeed(options: {
  feed: unknown;
  repo: string;
  version: string;
  publicKey: string;
  download: (url: string) => Promise<Uint8Array>;
}): Promise<string[]> {
  const problems: string[] = [];
  const feed = options.feed as Partial<Feed> | null;
  if (!feed || typeof feed !== "object") return ["the feed is not a JSON object"];
  if (feed.version !== options.version) {
    problems.push(`the feed announces ${String(feed.version)}, not ${options.version}`);
  }
  const names = updaterAssets(options.version);
  for (const key of Object.keys(names) as PlatformKey[]) {
    const entry = feed.platforms?.[key];
    if (!entry) {
      problems.push(`${key}: missing from the feed`);
      continue;
    }
    const expected = assetUrl(options.repo, options.version, names[key]);
    if (entry.url !== expected) {
      problems.push(`${key}: the feed points at ${entry.url}, expected ${expected}`);
      continue;
    }
    let data: Uint8Array;
    try {
      data = await options.download(entry.url);
    } catch (error) {
      problems.push(`${key}: cannot download ${entry.url} (${(error as Error).message})`);
      continue;
    }
    try {
      const { signedVersion } = verifyMinisign({
        publicKey: options.publicKey,
        signature: entry.signature,
        data,
      });
      // The current Tauri CLI records no version in the signature; when a signer does, it must be this one.
      if (signedVersion !== null && signedVersion !== options.version) {
        problems.push(
          `${key}: the signature was made for version ${signedVersion}, not ${options.version}`
        );
      }
    } catch (error) {
      problems.push(`${key}: ${(error as Error).message}`);
    }
  }
  return problems;
}
