# file-marks

Writes a note after a file or folder name in the project tree, remembered per
project, and lists those notes in a tool window.

中文简介：在项目树里给文件或文件夹的名字后面加一句备注（右键 → File Marks），
按项目保存、重启仍在；File Marks 工具窗口汇总所有打开项目的备注，可过滤、
可树形/扁平切换，行上右键可清除。

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
which project it belongs to. A project whose window is closed is not listed, and
a mark on a file that has stopped existing is dropped. Marks are kept in the
plugin's storage and come back when the IDE restarts.

Grant: `project.read`, `storage`. Needs the plugin API 1.10, so it installs on
OpenIDEA 0.1.47 and on anything newer.

## What this plugin does not do

The IDE it runs against offers a plugin one decoration record per file per
plugin, and each call writes a whole record — so a colour on the name and a note
after it cannot both be shown, and the second would hide the first. The note is
what stays. Likewise, the plugin API has no way to divide a context menu into
groups, so there is no line between the two entries.

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
plugins/build.sh plugins/community/file-marks
cargo test --manifest-path plugins/community/file-marks/Cargo.toml
```

Install it through Settings › Plugins › Install from Disk — the plugin folder, or
a `.zip` of it (`plugin.toml` and `dist/plugin.wasm` at the root, `target/` and
`sdk/` left out). See the [plugin guide](../../README.md).
