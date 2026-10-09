import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { buildNotes, type Commit } from "./notes.ts";

const c = (subject: string, body = ""): Commit => ({ subject, body });
const base = {
  version: "2.1.0",
  sha: "abc123",
  downloads: [["macOS", "MoliWhisper_2.1.0_macos_arm64.dmg"]] as [string, string][],
};

describe("buildNotes", () => {
  it("groups user-visible commits and drops the rest", () => {
    const notes = buildNotes({
      ...base,
      commits: [
        c("feat(overlay): show the organized text (#5)"),
        c("fix: keep the window on screen"),
        c("perf(audio): fewer copies"),
        c("refactor: split the session"),
        c("chore(release): v2.1.0"),
        c("docs: update the readme"),
        c("ci: add a job"),
        c("Bump vite from 1 to 2"),
      ],
    });
    assert.ok(notes.includes("## 更新内容"));
    assert.ok(notes.includes("**✨ 新功能**\n\n- overlay：show the organized text\n"));
    assert.ok(notes.includes("- keep the window on screen"));
    assert.ok(notes.includes("**⚡ 优化**\n\n- audio：fewer copies\n- split the session"));
    assert.ok(!notes.includes("readme"));
    assert.ok(!notes.includes("Bump"));
    assert.ok(!notes.includes("(#5)"));
    assert.ok(!notes.includes("不兼容"));
  });

  it("puts breaking changes first with what to do", () => {
    const notes = buildNotes({
      ...base,
      commits: [
        c("feat!: new settings format"),
        c("fix: small", "BREAKING CHANGE: enter the API key again after updating\n\nmore text"),
        c("feat: other"),
      ],
    });
    assert.ok(notes.indexOf("不兼容的改动") < notes.indexOf("## 更新内容"));
    assert.ok(notes.includes("- new settings format\n"));
    assert.ok(notes.includes("- small：enter the API key again after updating"));
    assert.ok(notes.includes("- other"));
  });

  it("always has the required sections, even with nothing to say", () => {
    const notes = buildNotes({ ...base, commits: [c("docs: x")] });
    for (const heading of ["## 更新内容", "## 下载", "## 首次安装", "## 校验"]) {
      assert.ok(notes.includes(heading), heading);
    }
    assert.ok(notes.includes("没有用户能看到的变化"));
    assert.ok(notes.includes("| macOS | `MoliWhisper_2.1.0_macos_arm64.dmg` |"));
    assert.ok(notes.includes("对应提交 abc123"));
    assert.ok(notes.includes("SHA256SUMS"));
    assert.ok(notes.includes("仍要打开"));
  });
});
