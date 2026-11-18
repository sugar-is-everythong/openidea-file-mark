//! File Marks: write a note after a file's name in the project tree.
//!
//! One mark is one line of text on one path, kept per project. The `file-marks`
//! tool window lists every mark of every open project, narrowed by a word and
//! shown either top to bottom or along the file structure.
//!
//! A mark is the note only. The tree of the IDE this plugin runs against keeps
//! one decoration record per file per plugin, and each call writes a whole one,
//! so a colour on the name and a note after it cannot both show: marking a file
//! with both would hide whichever was written first.

use std::collections::{BTreeMap, BTreeSet};

use openidea_plugin::{
    host,
    host::storage,
    register_plugin,
    views::{Body, Button, TreeItem, View, ViewEvent, Viewport, Views},
    Context, Error, FileKind, Invocation, Plugin, Project,
};

/// The set a mark is drawn with.
const SET: &str = "file-marks";
const VIEW: &str = "file-marks";
/// `fm:<project root>:<path relative to that root>`; the value is the remark.
const KEY_PREFIX: &str = "fm:";
/// Tool-window rows are keyed `kind:path`: the IDE reads the kind back as the
/// `viewItemKind` context key and passes the whole key as a command argument. A
/// folder shown only to hold the structure is the other kind, so its menu has
/// nothing to clear.
const MARK: &str = "mark";
const DIR: &str = "dir";
const BTN_FILTER: &str = "filter";
const BTN_CLEAR_FILTER: &str = "clear-filter";
const BTN_MODE: &str = "mode";
/// A remark is one line of at most this many characters, which is what
/// [`host::set_file_note`] draws; asking for more would silently lose it.
const MAX_REMARK: usize = 200;

#[derive(Default)]
struct FileMarks {
    /// A mark is listed when its path or remark holds this, ignoring case.
    filter: String,
    /// List the marks top to bottom instead of along the file structure.
    flat: bool,
    /// The folder rows the user closed, by key.
    closed: BTreeSet<String>,
}

register_plugin!(FileMarks, views);

impl Plugin for FileMarks {
    fn activate(&mut self, _ctx: &Context) -> Result<(), Error> {
        // Marks do not survive a restart, so rebuild them from storage.
        refresh()
    }

    fn run_command(&mut self, id: &str, ctx: &Invocation, arg: Option<&str>) -> Result<Option<String>, Error> {
        match id {
            "add-remark" => self.add_remark(ctx, arg),
            "clear-mark" => self.clear(ctx, arg),
            _ => Err(Error::NotFound(format!("command {id}"))),
        }
    }
}

impl Views for FileMarks {
    fn render(&mut self, view: &str, _viewport: Option<Viewport>, _ctx: &Invocation) -> Result<View, Error> {
        if view != VIEW {
            return Err(Error::NotFound(format!("view {view}")));
        }
        let all = listed()?;
        let shown: Vec<&Entry> = all.iter().filter(|entry| self.matches(entry)).collect();
        let mut view = if all.is_empty() {
            View::message("No marks yet.\n\nRight-click a file or folder in the project tree\nand pick File Marks.")
        } else if shown.is_empty() {
            View::message(&format!(
                "No mark holds \"{}\".\n\nPress × in the toolbar to list every mark again.",
                self.filter
            ))
        } else {
            let items = if self.flat { flat(&shown) } else { tree(&shown, &self.closed) };
            View::new(Body::Tree(items))
        };
        // The toolbar rides on every body: a filter that matches nothing has to
        // stay clearable from here, or the list is a dead end.
        let filter = if self.filter.is_empty() { "Filter…".to_owned() } else { format!("Filter: {}", self.filter) };
        view = view.button(Button::new(BTN_FILTER, &filter).icon("filter"));
        if !self.filter.is_empty() {
            view = view.button(Button::new(BTN_CLEAR_FILTER, "Clear filter").icon("close"));
        }
        let mode = if self.flat { "Tree" } else { "Flat" };
        view = view.button(Button::new(BTN_MODE, mode).icon(if self.flat { "structure" } else { "list" }));
        if !all.is_empty() {
            let status = if self.filter.is_empty() {
                format!("{} mark{}", all.len(), plural(all.len()))
            } else {
                format!("{} of {} marks match", shown.len(), all.len())
            };
            view = view.status(&status);
        }
        Ok(view)
    }

    fn handle(&mut self, view: &str, event: ViewEvent, _ctx: &Invocation) -> Result<(), Error> {
        if view != VIEW {
            return Err(Error::NotFound(format!("view {view}")));
        }
        match event {
            ViewEvent::Activate(key) => {
                if let Some(path) = unkey(&key, MARK) {
                    // A folder has no editor to open; the row is there to walk.
                    if !is_dir(&path) {
                        host::open_editor(&path, None)?;
                    }
                }
            }
            ViewEvent::Button(button) if button == BTN_FILTER => {
                // Emptying the box and answering is how a filter comes off.
                if let Some(text) = host::input("List only marks whose path or remark holds:", Some(&self.filter))? {
                    self.filter = text.trim().to_owned();
                }
                host::refresh_view(VIEW);
            }
            ViewEvent::Button(button) if button == BTN_CLEAR_FILTER => {
                self.filter.clear();
                host::refresh_view(VIEW);
            }
            ViewEvent::Button(button) if button == BTN_MODE => {
                self.flat = !self.flat;
                host::refresh_view(VIEW);
            }
            ViewEvent::Expand(key) => {
                self.closed.remove(&key);
            }
            ViewEvent::Collapse(key) => {
                self.closed.insert(key);
            }
            _ => {}
        }
        Ok(())
    }
}

impl FileMarks {
    /// Writes one remark on every path the call names, answering with what the
    /// first of them already said so a single file is edited rather than replaced.
    fn add_remark(&mut self, ctx: &Invocation, arg: Option<&str>) -> Result<Option<String>, Error> {
        let (project, paths) = targets(ctx, arg)?;
        let current = remark(&project, &paths[0])?;
        let Some(text) = host::input("Remark for this file or folder:", Some(&current))? else {
            return Ok(None);
        };
        let text = text.trim().to_owned();
        if text.len() > MAX_REMARK {
            return Err(Error::Invalid(format!("a remark is at most {MAX_REMARK} characters")));
        }
        for path in &paths {
            save(&project, path, &text)?;
        }
        let named = named(&project, &paths);
        refresh()?;
        Ok(Some(if text.is_empty() {
            format!("Took the remark off {named}.")
        } else {
            format!("Remark on {named}: {text}")
        }))
    }

    /// Takes the mark off every path the call names.
    fn clear(&mut self, ctx: &Invocation, arg: Option<&str>) -> Result<Option<String>, Error> {
        let (project, paths) = targets(ctx, arg)?;
        let mut cleared = 0;
        for path in &paths {
            if remark(&project, path)?.is_empty() {
                continue;
            }
            storage::delete(&storage_key(&project, path)?)?;
            cleared += 1;
        }
        refresh()?;
        let named = named(&project, &paths);
        Ok(Some(if cleared == 0 {
            format!("{named} carries no mark.")
        } else {
            format!("Cleared {cleared} mark{} from {named}.", plural(cleared))
        }))
    }

    fn matches(&self, entry: &Entry) -> bool {
        let needle = self.filter.to_lowercase();
        needle.is_empty() || entry.rel.to_lowercase().contains(&needle) || entry.remark.to_lowercase().contains(&needle)
    }
}

/// One listed mark: where it is, in which project, and what it says.
struct Entry {
    /// The project's own name, for a row of it to say which project it is in.
    project: String,
    /// The project root, as the tree needs it to walk down to a mark.
    root: String,
    path: String,
    rel: String,
    dir: bool,
    remark: String,
}

/// This project's marks, in path order. A marked path that has stopped existing
/// is forgotten here, so the tree and the list never show a dead entry.
fn marks(project: &Project) -> Result<Vec<Entry>, Error> {
    let root = project.root()?;
    let prefix = format!("{KEY_PREFIX}{root}:");
    let name = base_name(&root);
    let mut live = Vec::new();
    let mut gone = Vec::new();
    for key in storage::keys(&prefix)? {
        let Some(rel) = key.strip_prefix(&prefix).filter(|rel| !rel.is_empty()) else { continue };
        let rel = rel.replace('\\', "/");
        let full = join(&root, &rel);
        // Project lookup, not `host::fs::*`: that is for paths outside projects.
        match project.stat(&full) {
            Ok(stat) => live.push(Entry {
                project: name.clone(),
                root: root.clone(),
                remark: storage::get(&key)?.unwrap_or_default(),
                path: full,
                dir: stat.kind == FileKind::Directory,
                rel,
            }),
            Err(_) => gone.push(key),
        }
    }
    for key in gone {
        let _ = storage::delete(&key);
    }
    live.sort_by_key(|entry| entry.rel.to_lowercase());
    Ok(live)
}

/// Every mark of every open project, grouped by project and then in path order.
fn listed() -> Result<Vec<Entry>, Error> {
    let mut all = Vec::new();
    for project in host::projects()? {
        all.extend(marks(&project)?);
    }
    all.sort_by(|a, b| (&a.project, &a.rel).cmp(&(&b.project, &b.rel)));
    Ok(all)
}

/// Re-applies every mark of every open project.
fn refresh() -> Result<(), Error> {
    // Clearing takes the set out of every project at once, so every project has
    // to be re-applied in the same pass — one alone would hide the others.
    host::clear_file_decorations(SET)?;
    for project in host::projects()? {
        for entry in marks(&project)? {
            apply(&entry);
        }
    }
    Ok(())
}

/// Puts one mark back on the tree. An empty remark has nothing to draw, which is
/// also how a mark comes off; a path can stop existing between the listing and
/// the mark, hence the discarded result.
fn apply(entry: &Entry) {
    if entry.remark.is_empty() {
        return;
    }
    let _ = host::set_file_note(&entry.path, SET, &entry.remark, None);
}

/// One row per mark, top to bottom: the note first, the place it marks after
/// it, so a scan of the list reads what was said before where.
fn flat(entries: &[&Entry]) -> Vec<TreeItem> {
    // A list of one project needs no project in front of every path.
    let many = entries.iter().map(|entry| entry.project.as_str()).collect::<BTreeSet<_>>().len() > 1;
    entries
        .iter()
        .map(|entry| {
            let place = if many { format!("{}/{}", entry.project, entry.rel) } else { entry.rel.clone() };
            TreeItem::new(&view_key(MARK, &entry.path), &entry.remark)
                .icon(if entry.dir { "folder" } else { "file" })
                .detail(&place)
        })
        .collect()
}

/// The marks along the file structure: the folders a mark sits in appear as rows
/// too, so a deep path reads the way it lies on disk. `entries` are in path
/// order, which is why a folder is always in hand before anything inside it.
///
/// A row is keyed by the path it stands for exactly as that project spells it,
/// because the answer comes back as a command argument and has to name the file
/// to the project that holds it.
fn tree(entries: &[&Entry], closed: &BTreeSet<String>) -> Vec<TreeItem> {
    let marked = entries.iter().map(|entry| entry.path.clone()).collect::<BTreeSet<_>>();
    let mut rows = Rows { items: Vec::new(), at: BTreeMap::new(), closed, marked };
    for entry in entries {
        let mut parent = rows.open(&entry.root, &entry.project, true, None);
        let mut folder = entry.root.clone();
        let parts: Vec<&str> = entry.rel.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            folder = join(&folder, part);
            parent = rows.open(&folder, part, true, Some(parent));
        }
        let label = parts.last().copied().unwrap_or(&entry.rel);
        let leaf = rows.open(&entry.path, label, entry.dir, Some(parent));
        rows.items[leaf].detail = Some(entry.remark.clone());
    }
    rows.items
}

/// The rows of the tree, each keyed by the path it stands for.
struct Rows<'a> {
    items: Vec<TreeItem>,
    at: BTreeMap<String, usize>,
    closed: &'a BTreeSet<String>,
    /// The paths that carry a mark, so a marked folder is one row that can be
    /// cleared rather than a structure row and a mark row.
    marked: BTreeSet<String>,
}

impl Rows<'_> {
    /// The row of `path`, made when it is first needed; its key is decided there
    /// and never changes, because the rows below it hang from it.
    fn open(&mut self, path: &str, label: &str, folder: bool, parent: Option<usize>) -> usize {
        if let Some(index) = self.at.get(path) {
            return *index;
        }
        let key = if self.marked.contains(path) { view_key(MARK, path) } else { view_key(DIR, path) };
        let parent = parent.map(|index| self.items[index].key.clone());
        let mut item = TreeItem::new(&key, label).icon(if folder { "folder" } else { "file" });
        // A closed folder still lists its marks: they are all already here.
        if folder {
            item = item.expandable(!self.closed.contains(&key));
        }
        self.items.push(item);
        let index = self.items.len() - 1;
        if let Some(parent) = parent {
            self.items[index].parent = Some(parent);
        }
        self.at.insert(path.to_owned(), index);
        index
    }
}

/// The remark a path carries, empty when it carries none.
fn remark(project: &Project, path: &str) -> Result<String, Error> {
    Ok(storage::get(&storage_key(project, path)?)?.unwrap_or_default())
}

/// Keeps a remark, or forgets the record entirely when it says nothing: an empty
/// value is a mark that was taken off, not one to draw.
fn save(project: &Project, path: &str, remark: &str) -> Result<(), Error> {
    let key = storage_key(project, path)?;
    if remark.is_empty() {
        storage::delete(&key)
    } else {
        storage::set(&key, remark)
    }
}

/// The paths the call names, in the project they belong to: the tool-window row
/// that asked (`arg`), else the tree selection, else the open file. A row can
/// name a path the invoking window's own project does not hold.
fn targets(ctx: &Invocation, arg: Option<&str>) -> Result<(Project, Vec<String>), Error> {
    if let Some(path) = arg.and_then(|arg| unkey(arg, MARK)) {
        let project = host::project_of(&path).ok_or_else(|| Error::Invalid(format!("no project holds {path}")))?;
        return Ok((project, vec![path]));
    }
    let project = ctx.project().ok_or("no project is open")?;
    let mut paths = ctx.selected_paths();
    if paths.is_empty() {
        paths = ctx.editor().map(|editor| vec![editor.path()]).unwrap_or_default();
    }
    if paths.is_empty() {
        return Err("select a file or folder first".into());
    }
    Ok((project, paths))
}

/// How a command names what it acted on: one path by its own name, several by
/// how many they are.
fn named(project: &Project, paths: &[String]) -> String {
    match paths {
        [one] => rel_name(project, one),
        many => format!("{} files", many.len()),
    }
}

fn storage_key(project: &Project, path: &str) -> Result<String, Error> {
    Ok(format!("{}{}", prefix(project)?, rel_name(project, path)))
}

fn prefix(project: &Project) -> Result<String, Error> {
    Ok(format!("{KEY_PREFIX}{}:", project.root()?))
}

/// Whether `path` is a folder, as the tree of its own project draws it.
fn is_dir(path: &str) -> bool {
    host::project_of(path)
        .and_then(|project| project.stat(path).ok())
        .is_some_and(|stat| stat.kind == FileKind::Directory)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// `path` relative to the project root, with `/` separators.
fn rel_name(project: &Project, path: &str) -> String {
    let root = project.root().unwrap_or_default();
    path.strip_prefix(&root).unwrap_or(path).trim_start_matches(['/', '\\']).replace('\\', "/")
}

/// Joins a `/`-separated relative path onto a root that may use either
/// separator, keeping the root's own separator.
fn join(root: &str, rel: &str) -> String {
    let separator = if root.contains('\\') { '\\' } else { '/' };
    let rel = rel.replace('/', &separator.to_string());
    format!("{}{separator}{rel}", root.trim_end_matches(['/', '\\']))
}

/// The last segment of a path, with either separator.
fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rfind(['/', '\\']).map_or_else(|| trimmed.to_owned(), |i| trimmed[i + 1..].to_owned())
}

/// A row's key: the kind, then the path exactly as its project spells it, so
/// the answer names the file back to the project that holds it.
fn view_key(kind: &str, value: &str) -> String {
    format!("{kind}:{value}")
}

/// The value of a `kind:value` key, `None` for another kind.
fn unkey(key: &str, kind: &str) -> Option<String> {
    let (head, rest) = key.split_once(':')?;
    (head == kind).then(|| rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    const FILE: &str = "/project/src/main.rs";

    /// The paths the installed host has open, as the editor would show them.
    fn open_paths() -> Vec<String> {
        openidea_plugin::host::open_editors().unwrap().into_iter().map(|editor| editor.path).collect()
    }

    fn remark(host: &MockHost, path: &str) -> Option<String> {
        host.file_notes().get(&(SET.to_owned(), path.to_owned())).map(|(note, _)| note.clone())
    }

    /// The list one row per mark, which is the mode the view does not open in.
    fn show_flat(plugin: &mut FileMarks, host: &MockHost) {
        plugin.handle(VIEW, ViewEvent::Button(BTN_MODE.into()), &host.invocation()).unwrap();
    }

    fn rows(view: &View) -> Vec<TreeItem> {
        match &view.body {
            Body::Tree(items) => items.clone(),
            other => panic!("not a tree: {other:?}"),
        }
    }

    #[test]
    fn a_remark_reaches_the_tree_and_is_kept() {
        let host = MockHost::install();
        host.add_file(FILE, "fn main() {}\n");
        host.select_paths(&[FILE]);
        let mut plugin = FileMarks::default();

        host.answer_next_prompt(Some(&["the entry point"]));
        let out = plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Remark on src/main.rs: the entry point"));
        assert_eq!(remark(&host, FILE).as_deref(), Some("the entry point"));
        assert_eq!(host.storage()[&format!("{KEY_PREFIX}/project:src/main.rs")], "the entry point");
    }

    #[test]
    fn one_remark_answers_for_every_selected_file() {
        let host = MockHost::install();
        host.add_file("/project/a.rs", "x");
        host.add_file("/project/b.rs", "x");
        host.select_paths(&["/project/a.rs", "/project/b.rs"]);
        let mut plugin = FileMarks::default();

        host.answer_next_prompt(Some(&["work in progress"]));
        let out = plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Remark on 2 files: work in progress"));
        for path in ["/project/a.rs", "/project/b.rs"] {
            assert_eq!(remark(&host, path).as_deref(), Some("work in progress"));
        }
    }

    #[test]
    fn emptying_the_box_is_how_a_remark_comes_off() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        host.answer_next_prompt(Some(&[""]));
        let out = plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Took the remark off src/main.rs."));
        assert_eq!(remark(&host, FILE), None);
        assert!(host.storage().is_empty(), "a mark that says nothing is not kept");
    }

    #[test]
    fn clearing_takes_the_mark_off() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["mine"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        let out = plugin.run_command("clear-mark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Cleared 1 mark from src/main.rs."));
        assert!(host.storage().is_empty(), "an empty mark is not stored");
        assert_eq!(remark(&host, FILE), None);

        let out = plugin.run_command("clear-mark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("src/main.rs carries no mark."));
    }

    #[test]
    fn a_remark_too_long_to_draw_is_refused_before_it_is_kept() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        host.select_paths(&[FILE]);
        let too_long = "x".repeat(MAX_REMARK + 1);
        host.answer_next_prompt(Some(&[too_long.as_str()]));
        let error = FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
        assert!(host.storage().is_empty());
    }

    #[test]
    fn a_command_needs_something_to_mark() {
        let host = MockHost::install();
        for command in ["add-remark", "clear-mark"] {
            let error = FileMarks::default().run_command(command, &host.invocation(), None).unwrap_err();
            assert_eq!(error.to_string(), "select a file or folder first");
        }
    }

    #[test]
    fn restarting_reapplies_the_marks_of_every_open_project() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["one"]));
        FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap();
        // The other window's mark as that window would have saved it: the mock
        // draws a tree path only in the one project an invocation is rooted in.
        storage::set(&format!("{KEY_PREFIX}/other:b.rs"), "two").unwrap();

        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, FILE).as_deref(), Some("one"));

        host.set_project_root(Some("/other"));
        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, "/other/b.rs").as_deref(), Some("two"));
        assert_eq!(host.storage().len(), 2, "neither project's mark was taken for a dead one");
    }

    #[test]
    fn a_deleted_file_forgot_its_mark() {
        let host = MockHost::install();
        storage::set(&format!("{KEY_PREFIX}/project:gone.rs"), "bye").unwrap();
        FileMarks::default().activate(&Context::default()).unwrap();
        assert!(host.storage().is_empty(), "the dead record is dropped");
        assert!(host.file_notes().is_empty());
    }

    #[test]
    fn the_flat_list_puts_the_note_before_the_place() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        host.add_dir("/project/docs");
        let mut plugin = FileMarks::default();
        for (path, text) in [(FILE, "keep out"), ("/project/docs", "hand me in")] {
            host.select_paths(&[path]);
            host.answer_next_prompt(Some(&[text]));
            plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        }

        show_flat(&mut plugin, &host);
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        let items = rows(&view);
        // Path order, one row each, none hanging under another.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "hand me in");
        assert_eq!(items[0].detail.as_deref(), Some("docs"));
        assert_eq!(items[1].label, "keep out");
        assert_eq!(items[1].detail.as_deref(), Some("src/main.rs"));
        assert!(items.iter().all(|item| item.parent.is_none()));
        assert_eq!(view.status.as_deref(), Some("2 marks"));
        assert_eq!(view.toolbar[0].label, "Filter…");
        assert_eq!(view.toolbar[1].label, "Tree", "the button names the other list");
    }

    #[test]
    fn a_flat_list_of_several_projects_says_which_project_each_place_is_in() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        let mut plugin = FileMarks::default();
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["mine"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        // The other window's mark as that window would have saved it.
        storage::set(&format!("{KEY_PREFIX}/other:b.rs"), "theirs").unwrap();
        show_flat(&mut plugin, &host);

        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "theirs");
        assert_eq!(items[0].detail.as_deref(), Some("other/b.rs"));
        assert_eq!(items[1].label, "mine");
        assert_eq!(items[1].detail.as_deref(), Some("project/src/main.rs"));
    }

    #[test]
    fn the_tree_shows_the_folders_a_mark_sits_in() {
        let host = MockHost::install();
        host.add_file("/project/src/app/main.rs", "x");
        host.add_file("/project/src/lib.rs", "x");
        let mut plugin = FileMarks::default();
        for (path, text) in [("/project/src/app/main.rs", "the entry"), ("/project/src/lib.rs", "mine")] {
            host.select_paths(&[path]);
            host.answer_next_prompt(Some(&[text]));
            plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        }

        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        // The project, then the folders along the way, each appearing once.
        assert_eq!(labels, ["project", "src", "app", "main.rs", "lib.rs"]);
        // Named before the borrow: each row's key is what the row below it hangs from.
        let (project_row, src_row, app_row) = (items[0].key.clone(), items[1].key.clone(), items[2].key.clone());
        assert_eq!(project_row, view_key(DIR, "/project"));
        assert_eq!(items[2].parent.as_deref(), Some(src_row.as_str()), "app hangs under src");
        assert_eq!(items[3].parent.as_deref(), Some(app_row.as_str()));
        assert_eq!(items[4].parent.as_deref(), Some(src_row.as_str()), "src lists once");
        assert_eq!(items[3].key, view_key(MARK, "/project/src/app/main.rs"));
        assert_eq!(items[3].detail.as_deref(), Some("the entry"), "the row says what the mark says");
        assert_eq!(items[1].key, view_key(DIR, "/project/src"), "a folder only passed through");
        assert!(!items[3].expandable, "a file has no rows under it");
        assert!(items[1].expandable && items[1].expanded, "an open folder shows its marks");
    }

    #[test]
    fn a_closed_folder_stays_closed_and_its_marks_are_listed_when_opened() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        let src = view_key(DIR, "/project/src");

        plugin.handle(VIEW, ViewEvent::Collapse(src.clone()), &host.invocation()).unwrap();
        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        let folder = items.iter().find(|item| item.key == src).unwrap();
        assert!(folder.expandable && !folder.expanded);

        plugin.handle(VIEW, ViewEvent::Expand(src.clone()), &host.invocation()).unwrap();
        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        assert!(items.iter().find(|item| item.key == src).unwrap().expanded);
    }

    #[test]
    fn the_filter_narrows_the_list_and_says_by_how_much() {
        let host = MockHost::install();
        host.add_file("/project/a.rs", "x");
        host.add_file("/project/b.rs", "x");
        let mut plugin = FileMarks::default();
        for (path, text) in [("/project/a.rs", "keep out"), ("/project/b.rs", "settled")] {
            host.select_paths(&[path]);
            host.answer_next_prompt(Some(&[text]));
            plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        }
        show_flat(&mut plugin, &host);

        // The word matches a remark or a path.
        host.answer_next_prompt(Some(&["keep"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert_eq!(rows(&view).len(), 1);
        assert_eq!(view.status.as_deref(), Some("1 of 2 marks match"));
        assert_eq!(view.toolbar[0].label, "Filter: keep");
        assert_eq!(view.toolbar[1].id, BTN_CLEAR_FILTER, "the word on screen has its own way off");

        host.answer_next_prompt(Some(&["b.rs"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        assert_eq!(items[0].detail.as_deref(), Some("b.rs"), "the path still names the row, after the note");

        host.answer_next_prompt(Some(&["nothing holds this"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert!(matches!(&view.body, Body::Message(text) if text.contains("nothing holds this")));

        // The × clears the word without asking anything, even from this message.
        plugin.handle(VIEW, ViewEvent::Button(BTN_CLEAR_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert_eq!(rows(&view).len(), 2);
        assert_eq!(view.status.as_deref(), Some("2 marks"));
        assert_eq!(view.toolbar[0].label, "Filter…");
        assert!(view.toolbar.iter().all(|button| button.id != BTN_CLEAR_FILTER), "nothing left to clear");

        // Answering the box with nothing is the other way a filter comes off.
        host.answer_next_prompt(Some(&["keep"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        host.answer_next_prompt(Some(&[""]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert_eq!(rows(&view).len(), 2);
    }

    #[test]
    fn a_filter_matching_nothing_keeps_its_toolbar() {
        let host = MockHost::install();
        host.add_file("/project/a.rs", "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&["/project/a.rs"]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        host.answer_next_prompt(Some(&["no such word"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert!(matches!(view.body, Body::Message(_)));
        let ids: Vec<&str> = view.toolbar.iter().map(|button| button.id.as_str()).collect();
        assert_eq!(ids, [BTN_FILTER, BTN_CLEAR_FILTER, BTN_MODE], "the way out stays on screen");
        assert_eq!(view.status.as_deref(), Some("0 of 1 marks match"));
    }

    #[test]
    fn escaping_the_filter_leaves_it_alone() {
        let host = MockHost::install();
        host.add_file("/project/a.rs", "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&["/project/a.rs"]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        show_flat(&mut plugin, &host);

        host.answer_next_prompt(Some(&["keep"]));
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        plugin.render(VIEW, None, &host.invocation()).unwrap();

        // Escape answers with nothing at all, which is not an empty word.
        host.answer_next_prompt(None);
        plugin.handle(VIEW, ViewEvent::Button(BTN_FILTER.into()), &host.invocation()).unwrap();
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert_eq!(rows(&view).len(), 1, "still filtered, nothing was typed");
        assert_eq!(view.toolbar[0].label, "Filter: keep");
    }

    #[test]
    fn the_mode_button_switches_between_the_two_lists() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[FILE]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        // Tree first: the mark hangs under its folders.
        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        assert_eq!(items.len(), 3);
        assert_eq!(plugin.handle(VIEW, ViewEvent::Button(BTN_MODE.into()), &host.invocation()), Ok(()));
        let items = rows(&plugin.render(VIEW, None, &host.invocation()).unwrap());
        assert_eq!(items.len(), 1, "one row per mark, nothing in between");
        assert_eq!(items[0].label, "keep out");
        assert_eq!(items[0].detail.as_deref(), Some("src/main.rs"));
        assert_eq!(host.refreshed_views(), vec![VIEW.to_owned()]);
    }

    #[test]
    fn a_row_opens_its_file_and_a_folder_row_walks_instead() {
        let host = MockHost::install();
        host.add_file(FILE, "x");
        host.add_dir("/project/docs");
        let mut plugin = FileMarks::default();
        for path in [FILE, "/project/docs"] {
            host.select_paths(&[path]);
            host.answer_next_prompt(Some(&["keep out"]));
            plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        }

        plugin.handle(VIEW, ViewEvent::Activate(view_key(DIR, "/project/src")), &host.invocation()).unwrap();
        assert!(open_paths().is_empty(), "a folder shown for structure opens nothing");
        plugin.handle(VIEW, ViewEvent::Activate(view_key(MARK, "/project/docs")), &host.invocation()).unwrap();
        assert!(open_paths().is_empty(), "a marked folder has no editor either");

        plugin.handle(VIEW, ViewEvent::Activate(view_key(MARK, FILE)), &host.invocation()).unwrap();
        assert_eq!(open_paths(), [FILE]);
    }

    #[test]
    fn a_row_of_another_project_is_cleared_in_its_own() {
        let host = MockHost::install();
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        host.set_project_root(Some("/other"));
        host.select_paths(&["/other/b.rs"]);
        let mut plugin = FileMarks::default();
        host.answer_next_prompt(Some(&["theirs"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        // The tool window lists every project, so a row names its own.
        let mut plugin = FileMarks::default();
        let out = plugin.run_command("clear-mark", &host.invocation(), Some(&view_key(MARK, "/other/b.rs"))).unwrap();
        assert_eq!(out.as_deref(), Some("Cleared 1 mark from b.rs."));
        assert!(host.storage().is_empty());
        assert_eq!(remark(&host, "/other/b.rs"), None);
    }

    #[test]
    fn another_view_or_command_is_not_ours() {
        let host = MockHost::install();
        let mut plugin = FileMarks::default();
        assert!(matches!(plugin.render("something-else", None, &host.invocation()), Err(Error::NotFound(_))));
        assert!(matches!(
            plugin.handle("something-else", ViewEvent::Activate(String::new()), &host.invocation()),
            Err(Error::NotFound(_))
        ));
        let error = plugin.run_command("set-colour", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::NotFound(_)));
    }
}
