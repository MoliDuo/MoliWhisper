// Release notes from commit messages (standard 006, 6.7): Chinese headings, one fixed template.

export interface Commit {
  subject: string;
  body: string;
}

interface Parsed {
  type: string;
  scope: string | null;
  breaking: boolean;
  text: string;
  breakingNote: string | null;
}

const CONVENTIONAL = /^(\w+)(?:\(([^)]+)\))?(!)?: (.+)$/;

function parse(commit: Commit): Parsed | null {
  const match = CONVENTIONAL.exec(commit.subject);
  if (!match) return null;
  const note = /^BREAKING[ -]CHANGE: ([\s\S]+)$/m.exec(commit.body);
  return {
    type: match[1]!.toLowerCase(),
    scope: match[2] ?? null,
    breaking: match[3] === "!" || note !== null,
    // Squash-merged subjects end with "(#12)"; the number means nothing to a reader of the notes.
    text: match[4]!.replace(/\s*\(#\d+\)$/, ""),
    breakingNote: note?.[1]?.split("\n\n")[0]?.trim() ?? null,
  };
}

const SECTIONS: [string[], string][] = [
  [["feat"], "✨ 新功能"],
  [["fix"], "🐛 修复"],
  [["perf", "refactor"], "⚡ 优化"],
];

export const FIRST_INSTALL = [
  "1. 打开 `.dmg`，把 MoliWhisper 拖进「应用程序」。",
  "2. 没有经过 Apple 公证：第一次打开被拦住时，到「系统设置 → 隐私与安全性」点「仍要打开」；或在终端执行 `xattr -dr com.apple.quarantine /Applications/MoliWhisper.app`。",
  "3. 按提示授予**辅助功能**（全局热键和自动粘贴）和**麦克风**权限。",
  "",
  "已经装了的不用下载：应用会自己提示更新，也可以在菜单栏点「检查更新…」。每个版本用同一张证书签名，更新后授权保留。",
].join("\n");

export function buildNotes(options: {
  commits: Commit[];
  version: string;
  sha: string;
  /** `[platform, asset name]` for the download table. */
  downloads: [string, string][];
}): string {
  const parsed = options.commits.map(parse).filter((p): p is Parsed => p !== null);
  const out: string[] = [];

  const breaking = parsed.filter((p) => p.breaking);
  if (breaking.length) {
    out.push("## ⚠️ 不兼容的改动", "");
    for (const p of breaking) out.push(`- ${p.text}${p.breakingNote ? `：${p.breakingNote}` : ""}`);
    out.push("");
  }

  out.push("## 更新内容", "");
  let any = false;
  for (const [types, heading] of SECTIONS) {
    const lines = parsed.filter((p) => types.includes(p.type) && !p.breaking);
    if (!lines.length) continue;
    any = true;
    out.push(`**${heading}**`, "");
    for (const p of lines) out.push(`- ${p.scope ? `${p.scope}：` : ""}${p.text}`);
    out.push("");
  }
  if (!any) out.push("这个版本没有用户能看到的变化（内部改进）。", "");

  out.push("## 下载", "", "| 平台 | 文件 |", "| --- | --- |");
  for (const [platform, file] of options.downloads) out.push(`| ${platform} | \`${file}\` |`);
  out.push("", "## 首次安装", "", FIRST_INSTALL, "");
  out.push(
    "## 校验",
    "",
    `对应提交 ${options.sha}；校验和见 \`SHA256SUMS\`（\`sha256sum -c SHA256SUMS\`）。`,
    ""
  );
  return out.join("\n");
}
