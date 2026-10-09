// Standard 006, 6.2 and 6.3: the tag must match the version file, and versions only go up.

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

const parts = (version: string) => (SEMVER.exec(version) ?? []).slice(1).map(Number);

/** Negative when `a` is older than `b`. */
export function compareVersions(a: string, b: string): number {
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return (x[i] ?? 0) - (y[i] ?? 0);
  return 0;
}

/** Problems with releasing `tag` when the version file says `packageVersion`; empty means go ahead. */
export function checkTag(tag: string, packageVersion: string, existingTags: string[]): string[] {
  const problems: string[] = [];
  const match = /^v(\d+\.\d+\.\d+)$/.exec(tag);
  if (!match) return [`the tag ${tag} is not vX.Y.Z`];
  const version = match[1]!;
  if (version !== packageVersion) {
    problems.push(`the tag says ${version} but package.json says ${packageVersion}`);
  }
  const newer = existingTags
    .filter((other) => other !== tag && /^v\d+\.\d+\.\d+$/.test(other))
    .filter((other) => compareVersions(other.slice(1), version) >= 0);
  if (newer.length) problems.push(`${tag} is not newer than ${newer.sort().join(", ")}`);
  return problems;
}

/** The newest `vX.Y.Z` tag older than `tag`, for the release notes' range; null for the first release. */
export function previousTag(tag: string, existingTags: string[]): string | null {
  const version = tag.replace(/^v/, "");
  const older = existingTags
    .filter(
      (other) => /^v\d+\.\d+\.\d+$/.test(other) && compareVersions(other.slice(1), version) < 0
    )
    .sort((a, b) => compareVersions(a.slice(1), b.slice(1)));
  return older.at(-1) ?? null;
}
