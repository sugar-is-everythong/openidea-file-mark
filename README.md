# file-marks

Writes a note after a file or folder name in the project tree, keeps the notes
in `file-mark.json` at the project root — a plain file, so a whole team shares
one set of them and they travel with the code — and lists them in a tool window.
Writing that file is the only file write the plugin does, and remarks are the
only thing in it.

中文简介：在项目树里给文件或文件夹的名字后面加一句备注（右键 → File Marks）；
备注存在项目根目录的 file-mark.json 里，跟着代码走，可以提交、可以忽略、也可以手改。
写文件只为了持久化备注这一个用途，除 file-mark.json 外本插件不写任何文件；之所以
写成项目里的文件、而不是插件私有存储，是为了多人开发时可迁移、可共享——提交进仓库，
同事 clone 下来就能看到同一份备注。
File Marks 工具窗口汇总所有打开项目的备注，可过滤、可树形/扁平切换，行上右键可清除。

Right-click a file or folder → **File Marks** →

- **Add Remark…** — the text after the name, one line of up to 200 characters;
  answering with nothing takes the note off. Several selected files get the
  same note; a long one is refused rather than cut short.
- **Clear** — takes the note off.

The **File Marks** tool window lists the marks of every open project, either in
the tree's own shape or as one flat list sorted by path, each row reading the
note first and the path after it — the toolbar button names the other one. **Filter…** narrows the list by path or note; while a word
is set, a **×** beside it clears the filter in one click, and the toolbar stays
on screen even when nothing matches. A row's context menu clears that mark. When more than one project has marks, a row says
which project it belongs to. A project whose window is closed is not listed.

## Where the notes live

`file-mark.json`, in the project's own root — one file per project, and the
whole of what the plugin keeps:

```json
{
  "version": 1,
  "marks": {
    "docs/design.md": "hand me in",
    "src/main.rs": "keep out"
  }
}
```

Paths are relative to that root and use `/`, sorted, so the file diffs one line
per change. It is a plain file, so it can be edited by hand: take a line out and
the note leaves the tree, add one and it comes back. Commit it and the notes
travel with a clone; name it in `.gitignore` and they stay on this machine.

Nothing is written on a look: the file changes when a remark is put on or taken
off, and a note whose path the project reports as missing goes out with that
write rather than on its own. A project that will not answer for a path — a busy
disk, an index not ready — keeps every note it holds, and the window says the
read failed instead of listing nothing, because a momentary failure is not the
note being taken off. The same holds for the file itself, which is read before
it is written and left alone when it cannot be read: a file written by a newer
File Marks, or one cut off mid-edit, is reported, never flattened.

## Why the plugin writes a file

The write is the design, not a side effect: `file-mark.json` is the only file it
creates or changes, a remark is the only thing ever in it, and it is written when
a remark goes on or comes off and at no other time. Keeping the notes in the
plugin's own private storage would ask for no such permission, but then a note
belongs to one machine and one user — it does not reach a clone, a second
checkout, or a teammate, and it cannot be reviewed or edited as part of the
repository. A file at the project's root is what makes a note shareable: commit
it and everyone working on the code reads the same notes, delete a line in a
branch and the note is gone for them too, and `git blame` says who put it there.
A project that wants its notes private names the same file in `.gitignore` and
loses nothing else.

So the grant is `project.read` (Low risk) to put a note on the tree, and
`project.write` (High risk) to keep this one file. Nothing else is asked for, and
nothing else is written. The install dialog calls a high-risk capability
equivalent to running code on the machine; what it buys here is one file the team
can share, and the write goes to `file-mark.json` and to no other path. Needs the
plugin API 1.10, so it installs on OpenIDEA 0.1.47 and on anything newer.

中文：写文件是本插件方案的核心，不是附带行为：它只创建和修改 `file-mark.json`
这一个文件，里面只放备注，只在加/去备注时写，浏览列表时一个字节都不写。备注存在
插件私有存储里同样能用，也不需要这个权限，但那样备注就只属于这台机器、这个用户：
进不了 clone，同事看不到，也不能作为仓库内容被评审和手改。放在项目根目录，备注
才可迁移、可共享：提交之后所有协作者看到的是同一份，某个分支删掉一行，别人那边也
就没了，`git blame` 还能看出是谁写的。想让备注只留在本机，把 `file-mark.json`
写进 `.gitignore` 就行，其它功能一点不受影响。因此申请的是 `project.read`
（低风险，用来把备注画到树上）和 `project.write`（高风险，只用来保存这一个文件），
别无其它。安装对话框会把高风险权限说成等同于在本机执行代码；这里换来的只是这一个
能被共享的备注文件，写入只发生在 `file-mark.json` 这一个路径上。

## What this plugin does not do

The IDE it runs against offers a plugin one decoration record per file per
plugin, and each call writes a whole record — so a colour on the name and a note
after it cannot both be shown, and the second would hide the first. The note is
what stays. Likewise, the plugin API has no way to divide a context menu into
groups, so there is no line between the two entries.

A plugin cannot delete a project's file, so taking the last note off leaves
`file-mark.json` behind with nothing in it. An edit made outside the IDE shows
in the list the next time it draws, and on the tree the next time a remark is
written or the IDE restarts: nothing watches the file.

## Building

The plugin builds against the SDK of a release, vendored under `sdk/`: a
component built against a newer WIT than the IDE has is refused at install, so
`Cargo.toml` points here rather than at the repository's own `sdk/`. To follow a
newer release, re-extract it from that tag:

```sh
rm -rf sdk && mkdir -p sdk
git archive <tag> sdk/rust/openidea-plugin | tar -x -C sdk
mv sdk/sdk/rust/openidea-plugin sdk/openidea-plugin && rm -rf sdk/sdk
```

From the repository root, build and test:

```sh
cargo build --locked --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/openidea_plugin_openidea_file_marks.wasm dist/plugin.wasm
cargo test
```

The component lands in `dist/plugin.wasm`, which is gitignored but must exist
before packaging.

Install it through Settings › Plugins › Install from Disk — the plugin folder, a
`.zip` of it (`plugin.toml` and `dist/plugin.wasm` at the root, `target/` and
`sdk/` left out), or the signed
`sugar-is-everythong.file-marks-0.1.1.oiplugin` from a release.
See the [plugin guide](https://github.com/l0ng-ai/openidea/blob/main/plugins/README.md).
