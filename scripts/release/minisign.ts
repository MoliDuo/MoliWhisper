// Checks a Tauri update signature (minisign, Ed25519) the way the app's updater does, so the release
// pipeline can prove, with the public key that is shipped in the app, that what it published will install.
import { createHash, createPublicKey, verify } from "node:crypto";

const SPKI_ED25519_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

interface KeyBlock {
  comment: string;
  body: Buffer;
  rest: string[];
}

/** A minisign file is text; Tauri passes it around base64-encoded. */
function decodeFile(encoded: string, what: string): string[] {
  const text = Buffer.from(encoded.trim(), "base64").toString("utf-8");
  const lines = text.split("\n").map((line) => line.replace(/\r$/, ""));
  if (lines.length < 2 || !lines[0]?.startsWith("untrusted comment:")) {
    throw new Error(`${what} is not a minisign file`);
  }
  return lines;
}

function readBlock(encoded: string, what: string): KeyBlock {
  const lines = decodeFile(encoded, what);
  return {
    comment: lines[0] ?? "",
    body: Buffer.from(lines[1] ?? "", "base64"),
    rest: lines.slice(2),
  };
}

const ed25519 = (key: Buffer) =>
  createPublicKey({ key: Buffer.concat([SPKI_ED25519_PREFIX, key]), format: "der", type: "spki" });

export interface VerifiedSignature {
  /** The comment the signer attached and signed over (Tauri writes `timestamp`, `file` and `version`). */
  trustedComment: string;
  /** `version:` from the trusted comment, or null when the signer did not record one. */
  signedVersion: string | null;
}

/**
 * Throws unless `signature` (the base64 text of a `.sig` file, which is also what `latest.json` carries) is
 * a valid signature of `data` by `publicKey` (the base64 text from `plugins.updater.pubkey`).
 */
export function verifyMinisign(options: {
  publicKey: string;
  signature: string;
  data: Uint8Array;
}): VerifiedSignature {
  const pub = readBlock(options.publicKey, "the public key");
  if (pub.body.length !== 42 || pub.body.subarray(0, 2).toString() !== "Ed") {
    throw new Error("the public key is not an Ed25519 minisign key");
  }
  const sig = readBlock(options.signature, "the signature");
  if (sig.body.length !== 74) throw new Error("the signature has the wrong length");
  const algorithm = sig.body.subarray(0, 2).toString();
  if (algorithm !== "Ed" && algorithm !== "ED") throw new Error("unknown signature algorithm");
  if (!sig.body.subarray(2, 10).equals(pub.body.subarray(2, 10))) {
    throw new Error("the signature was made with a different key");
  }
  const key = ed25519(pub.body.subarray(10));
  // "ED" signs the BLAKE2b-512 hash of the file instead of the file itself.
  const message =
    algorithm === "ED" ? createHash("blake2b512").update(options.data).digest() : options.data;
  const fileSignature = sig.body.subarray(10);
  if (!verify(null, message, key, fileSignature)) {
    throw new Error("the signature does not match the file");
  }
  const trusted = sig.rest[0]?.replace(/^trusted comment: ?/, "") ?? "";
  const globalSignature = Buffer.from(sig.rest[1] ?? "", "base64");
  if (!verify(null, Buffer.concat([fileSignature, Buffer.from(trusted)]), key, globalSignature)) {
    throw new Error("the signature's comment was changed");
  }
  const signedVersion =
    trusted
      .split("\t")
      .find((field) => field.startsWith("version:"))
      ?.slice("version:".length) ?? null;
  return { trustedComment: trusted, signedVersion };
}
