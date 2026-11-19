//! File Marks: write a note after a file's name in the project tree.
//!
//! One mark is one line of text on one path. A project's notes are kept in
//! `file-mark.json` at its root, so they sit with the code they describe: they
//! can be committed, ignored, or edited by hand, and they come back with a
//! clone. The `file-marks` tool window lists every mark of every open project,
//! narrowed by a word and shown either top to bottom or along the file
//! structure.
//!
//! A mark is the note only. The tree of the IDE this plugin runs against keeps
//! one decoration record per file per plugin, and each call writes a whole one,
//! so a colour on the name and a note after it cannot both show: marking a file
//! with both would hide whichever was written first.

use std::collections::{BTreeMap, BTreeSet};

use openidea_plugin::{
    host, register_plugin,
    views::{Body, Button, TreeItem, View, ViewEvent, Viewport, Views},
    Context, Error, FileKind, FileStat, Invocation, Plugin, Project,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::cell::RefCell;

/// The set a mark is drawn with.
const SET: &str = "file-marks";
const VIEW: &str = "file-marks";
/// Where a project keeps its notes: this file, at its root.
const FILE: &str = "file-mark.json";
/// The shape of that file, written into it, so one made by a newer plugin is
/// refused rather than rewritten with only what this version understands.
const LAYOUT: u32 = 1;
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
        // The notes are in the project's file; what a restart loses is the
        // drawing, so that is all there is to put back.
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
    ///
    /// The file is read before the box opens: notes that cannot be written back
    /// should not be asked for in the first place.
    fn add_remark(&mut self, ctx: &Invocation, arg: Option<&str>) -> Result<Option<String>, Error> {
        let (project, paths) = targets(ctx, arg)?;
        let mut book = Book::load(&project)?;
        let current = book.remark(&project, &paths[0]);
        let Some(text) = host::input("Remark for this file or folder:", Some(&current))? else {
            return Ok(None);
        };
        let text = text.trim().to_owned();
        if text.len() > MAX_REMARK {
            return Err(Error::Invalid(format!("a remark is at most {MAX_REMARK} characters")));
        }
        for path in &paths {
            book.put(&rel_name(&project, path), &text);
        }
        book.save(&project)?;
        let named = named(&project, &paths);
        redraw(Some((&project, &book)))?;
        Ok(Some(if text.is_empty() {
            format!("Took the remark off {named}.")
        } else {
            format!("Remark on {named}: {text}")
        }))
    }

    /// Takes the mark off every path the call names. A project with nothing to
    /// take off is left without its file being touched, or made.
    fn clear(&mut self, ctx: &Invocation, arg: Option<&str>) -> Result<Option<String>, Error> {
        let (project, paths) = targets(ctx, arg)?;
        let mut book = Book::load(&project)?;
        let mut cleared = 0;
        for path in &paths {
            if book.put(&rel_name(&project, path), "") {
                cleared += 1;
            }
        }
        if cleared > 0 {
            book.save(&project)?;
        }
        redraw(Some((&project, &book)))?;
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

/// What one project keeps: each marked path, as it reads from the root, and the
/// note to write after its name.
#[derive(Deserialize, Serialize)]
struct Book {
    /// [`LAYOUT`], so a file this plugin would write shorter is refused instead.
    version: u32,
    /// Path to note, sorted: a written file then diffs one line per change.
    marks: BTreeMap<String, String>,
}

impl Book {
    /// The project's file. Having no file yet is having no marks; a file that
    /// cannot be read, or not read as this shape, is the error, and an error is
    /// never written over — a note lost to one bad read would be lost for good.
    fn load(project: &Project) -> Result<Book, Error> {
        let text = match read(project, &file(project)?) {
            Ok(text) => text,
            Err(Error::NotFound(_)) => return Ok(Book::empty()),
            Err(error) => return Err(error),
        };
        let mut book: Book = serde_json::from_str(&text).map_err(invalid)?;
        if book.version != LAYOUT {
            return Err(Error::Invalid(format!("{FILE}: layout {} is not one this plugin can read", book.version)));
        }
        // Keys are written with `/`; a hand-edited file may not be, and a note
        // on `src\main.rs` would otherwise never meet the file it marks.
        book.marks = book.marks.into_iter().map(|(rel, note)| (rel.replace('\\', "/"), note)).collect();
        book.marks.retain(|_, note| !note.is_empty());
        Ok(book)
    }

    fn empty() -> Book {
        Book { version: LAYOUT, marks: BTreeMap::new() }
    }

    /// The note `path` carries, empty when it carries none.
    fn remark(&self, project: &Project, path: &str) -> String {
        self.marks.get(&rel_name(project, path)).cloned().unwrap_or_default()
    }

    /// Puts a note on a path, or takes it off: an empty note is a mark that was
    /// removed, not one to draw. Whether anything changed comes back, because
    /// clearing a file that carries no mark has nothing to say for itself.
    fn put(&mut self, rel: &str, note: &str) -> bool {
        if note.is_empty() {
            self.marks.remove(rel).is_some()
        } else {
            self.marks.insert(rel.to_owned(), note.to_owned()).as_deref() != Some(note)
        }
    }

    /// Writes the file, dropping the marks whose paths the project reports as
    /// missing — the notes are pruned when the file is written for some reason
    /// anyway, not on a look at the list.
    fn save(&mut self, project: &Project) -> Result<(), Error> {
        let root = project.root()?;
        self.marks.retain(|rel, _| match stat(project, &join(&root, rel)) {
            // Only "missing" is a deleted file; anything else keeps the note.
            Err(Error::NotFound(_)) => false,
            _ => true,
        });
        let mut text = serde_json::to_string_pretty(self).map_err(invalid)?;
        text.push('\n');
        project.write_file(&file(project)?, &text)
    }
}

/// What a file this plugin cannot make of has to say, read or written.
fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{FILE}: {error}"))
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

/// This project's marks, in path order: what its file says, of the paths the
/// project still has. A mark whose path the project reports as missing is left
/// out here, so the tree and the list never show a dead entry, and `Book::save`
/// drops it for good when the file is next written. Any other answer is a read
/// that failed rather than a file that was deleted, so nothing is dropped and
/// the failure is the answer instead: an entry let go on a stumble would be
/// gone for good.
fn live(project: &Project, book: &Book) -> Result<(Vec<Entry>, Option<Error>), Error> {
    let root = project.root()?;
    let name = base_name(&root);
    let mut entries = Vec::new();
    let mut trouble = None;
    for (rel, remark) in &book.marks {
        let path = join(&root, rel);
        // Project lookup, not `host::fs::*`: that is for paths outside projects.
        match stat(project, &path) {
            Ok(info) => entries.push(Entry {
                project: name.clone(),
                root: root.clone(),
                remark: remark.clone(),
                dir: info.kind == FileKind::Directory,
                path,
                rel: rel.clone(),
            }),
            Err(Error::NotFound(_)) => {}
            Err(error) => {
                if trouble.is_none() {
                    trouble = Some(Error::Failed(format!("can't check the mark on {rel}: {error}")));
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.rel.to_lowercase());
    Ok((entries, trouble))
}

/// A project's marks as its own file holds them.
fn on_disk(project: &Project) -> Result<(Vec<Entry>, Option<Error>), Error> {
    live(project, &Book::load(project)?)
}

fn marks(project: &Project) -> Result<Vec<Entry>, Error> {
    let (entries, trouble) = on_disk(project)?;
    trouble.map_or(Ok(entries), Err)
}

/// What the project says about `path`. Reads come through here so a test can
/// hold one back: `MockHost` refuses a read for one reason only — the path is
/// missing — and that is the one reason this plugin may forget a mark.
fn stat(project: &Project, path: &str) -> Result<FileStat, Error> {
    #[cfg(test)]
    if held(path) {
        return Err(Error::Failed("the project would not answer".into()));
    }
    project.stat(path)
}

/// The text of a project file, held back the same way. What cannot be read is
/// never written over, so the notes file has the most to lose from a stumble.
fn read(project: &Project, path: &str) -> Result<String, Error> {
    #[cfg(test)]
    if let Some(text) = answered_stale(path) {
        return Ok(text);
    }
    #[cfg(test)]
    if held(path) {
        return Err(Error::Failed("the project would not answer".into()));
    }
    project.read_file(path)
}

// Paths whose project reads are held back, by the test that holds one; a doc
// comment would not survive the macro it hangs on.
#[cfg(test)]
thread_local! {
    static HELD: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    // …and paths whose read is answered for a file that no longer says that,
    // which is what a read made in the same pass as a write can do.
    static STALE: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
}

#[cfg(test)]
fn held(path: &str) -> bool {
    HELD.with(|held| held.borrow().contains(path))
}

#[cfg(test)]
fn answered_stale(path: &str) -> Option<String> {
    STALE.with(|stale| stale.borrow().get(path).cloned())
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

/// Re-applies every mark of every open project, read from the files that hold them.
fn refresh() -> Result<(), Error> {
    redraw(None)
}

/// Re-applies every mark, taking the notes of `written` — the project just
/// written to, from the book that was put in its file — from memory. A read
/// made in the same pass as a write answers for the file as it was before the
/// write, and a tree drawn from that answer shows the note one operation late.
fn redraw(written: Option<(&Project, &Book)>) -> Result<(), Error> {
    // Clearing takes the set out of every project at once, so every project has
    // to be re-applied in the same pass — one alone would hide the others.
    host::clear_file_decorations(SET)?;
    let mut trouble = None;
    for project in host::projects()? {
        let read = match written {
            Some((here, book)) if holds_the_same_project(here, &project) => live(&project, book),
            _ => on_disk(&project),
        };
        match read {
            Ok((entries, stalled)) => {
                for entry in entries {
                    apply(&entry);
                }
                if trouble.is_none() {
                    trouble = stalled;
                }
            }
            // A window whose project will not answer says so, but not at the
            // cost of the other windows' marks.
            Err(error) => {
                if trouble.is_none() {
                    trouble = Some(error);
                }
            }
        }
    }
    trouble.map_or(Ok(()), Err)
}

/// Whether both handle the same root.
fn holds_the_same_project(here: &Project, other: &Project) -> bool {
    match (here.root(), other.root()) {
        (Ok(here), Ok(other)) => here == other,
        _ => false,
    }
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

/// The file a project keeps its notes in: its own root, so the notes belong to
/// the code they describe.
fn file(project: &Project) -> Result<String, Error> {
    Ok(join(&project.root()?, FILE))
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

    const MAIN: &str = "/project/src/main.rs";

    /// The mock, with the one grant `install` does not hand out and this plugin
    /// needs: writing its file. What is missing from the list matters as much —
    /// `storage` is not there, so a plugin still asking for it would fail here.
    fn install() -> MockHost {
        MockHost::install().with_capabilities(&[
            "ide.commands",
            "project.read",
            "project.write",
            "editor.read",
            "editor.write",
            "vcs.read",
            "vcs.write",
            "build.read",
            "events",
            "events.input",
            "clipboard",
        ])
    }

    /// The project that mock says is open.
    fn project(root: &str) -> Project {
        host::projects()
            .unwrap()
            .into_iter()
            .find(|project| project.root().unwrap() == root)
            .unwrap_or_else(|| panic!("no project at {root}"))
    }

    /// The notes file of a project, as it stands: read past the plugin, since
    /// what a test holds back is often the plugin's own read.
    fn notes(root: &str) -> Option<String> {
        project(root).read_file(&format!("{root}/{FILE}")).ok()
    }

    /// The note the file keeps on `rel`.
    fn kept(root: &str, rel: &str) -> Option<String> {
        let text = notes(root)?;
        serde_json::from_str::<Book>(&text).ok()?.marks.get(rel).cloned()
    }

    /// What another window would have left behind: the project's file, holding
    /// these marks. Seeded rather than typed, because the mock keeps a written
    /// file under its root-relative name, which two projects would share.
    fn seed(host: &MockHost, root: &str, marks: &[(&str, &str)]) {
        host.add_file(&format!("{root}/{FILE}"), &text(marks));
    }

    /// The notes file as this plugin writes it with these marks.
    fn text(marks: &[(&str, &str)]) -> String {
        let book = Book {
            version: LAYOUT,
            marks: marks.iter().map(|(rel, note)| ((*rel).to_owned(), (*note).to_owned())).collect(),
        };
        let mut text = serde_json::to_string_pretty(&book).unwrap();
        text.push('\n');
        text
    }

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
    fn a_remark_reaches_the_tree_and_is_kept_in_the_project() {
        let host = install();
        host.add_file(MAIN, "fn main() {}\n");
        host.select_paths(&[MAIN]);
        let mut plugin = FileMarks::default();

        host.answer_next_prompt(Some(&["the entry point"]));
        let out = plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Remark on src/main.rs: the entry point"));
        assert_eq!(remark(&host, MAIN).as_deref(), Some("the entry point"));
        // The whole file, because it sits in the project for anyone to read:
        // one note per line, the path as the project sees it, sorted.
        assert_eq!(
            notes("/project").as_deref(),
            Some("{\n  \"version\": 1,\n  \"marks\": {\n    \"src/main.rs\": \"the entry point\"\n  }\n}\n")
        );
    }

    #[test]
    fn one_remark_answers_for_every_selected_file() {
        let host = install();
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
        for rel in ["a.rs", "b.rs"] {
            assert_eq!(kept("/project", rel).as_deref(), Some("work in progress"), "{rel} is in the file too");
        }
    }

    #[test]
    fn emptying_the_box_is_how_a_remark_comes_off() {
        let host = install();
        host.add_file(MAIN, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[MAIN]);
        host.answer_next_prompt(Some(&["keep out"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        host.answer_next_prompt(Some(&[""]));
        let out = plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Took the remark off src/main.rs."));
        assert_eq!(remark(&host, MAIN), None);
        assert_eq!(kept("/project", "src/main.rs"), None, "a note that says nothing is not kept");
        // The file itself stays, empty: taking the last note off is not the
        // plugin's business in someone else's project tree.
        assert_eq!(notes("/project").as_deref(), Some("{\n  \"version\": 1,\n  \"marks\": {}\n}\n"));
    }

    #[test]
    fn clearing_takes_the_mark_off() {
        let host = install();
        host.add_file(MAIN, "x");
        let mut plugin = FileMarks::default();
        host.select_paths(&[MAIN]);
        host.answer_next_prompt(Some(&["mine"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();

        let out = plugin.run_command("clear-mark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("Cleared 1 mark from src/main.rs."));
        assert_eq!(kept("/project", "src/main.rs"), None);
        assert_eq!(remark(&host, MAIN), None);

        let before = notes("/project");
        let out = plugin.run_command("clear-mark", &host.invocation(), None).unwrap();
        assert_eq!(out.as_deref(), Some("src/main.rs carries no mark."));
        assert_eq!(notes("/project"), before, "nothing to take off leaves the file as it was");
    }

    #[test]
    fn a_remark_too_long_to_draw_is_refused_before_it_is_kept() {
        let host = install();
        host.add_file(MAIN, "x");
        host.select_paths(&[MAIN]);
        let too_long = "x".repeat(MAX_REMARK + 1);
        host.answer_next_prompt(Some(&[too_long.as_str()]));
        let error = FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
        assert_eq!(notes("/project"), None, "a refused note does not add a file to the project");
    }

    #[test]
    fn a_command_needs_something_to_mark() {
        let host = install();
        for command in ["add-remark", "clear-mark"] {
            let error = FileMarks::default().run_command(command, &host.invocation(), None).unwrap_err();
            assert_eq!(error.to_string(), "select a file or folder first");
        }
    }

    #[test]
    fn restarting_reapplies_the_marks_of_every_open_project() {
        let host = install();
        host.add_file(MAIN, "x");
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        // Each project's own file, as the window that wrote it left it.
        seed(&host, "/project", &[("src/main.rs", "one")]);
        seed(&host, "/other", &[("b.rs", "two")]);

        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, MAIN).as_deref(), Some("one"));
        assert_eq!(kept("/project", "src/main.rs").as_deref(), Some("one"), "listing the marks never rewrites them");

        // The other window's file is read in the same pass; the mock only draws
        // a tree path in the project an invocation is rooted in, so that half
        // waits for the root to move.
        host.set_project_root(Some("/other"));
        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, "/other/b.rs").as_deref(), Some("two"));
        assert_eq!(kept("/other", "b.rs").as_deref(), Some("two"), "and neither file was touched here either");
    }

    /// A mark on a path that is gone has to leave, but the notes are a file the
    /// user owns now: looking at it is no reason to write it.
    #[test]
    fn a_deleted_file_loses_its_mark_when_the_file_is_written_again() {
        let host = install();
        host.add_file(MAIN, "x");
        seed(&host, "/project", &[("gone.rs", "bye"), ("src/main.rs", "kept")]);

        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, "/project/gone.rs"), None, "a dead path is not drawn");
        assert_eq!(remark(&host, MAIN).as_deref(), Some("kept"));
        assert_eq!(kept("/project", "gone.rs").as_deref(), Some("bye"), "and a look writes nothing");

        host.select_paths(&[MAIN]);
        host.answer_next_prompt(Some(&["kept"]));
        FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap();
        assert_eq!(kept("/project", "gone.rs"), None, "the dead note goes with the next write");
        assert_eq!(kept("/project", "src/main.rs").as_deref(), Some("kept"));
    }

    /// A project that will not answer for a path — a busy disk, a read refused,
    /// an index not ready — has said nothing about the file being gone. Losing
    /// the note there would be losing it for good, on a moment's trouble.
    #[test]
    fn a_read_that_stumbles_keeps_the_mark_and_says_so() {
        let host = install();
        host.add_file(MAIN, "x");
        host.select_paths(&[MAIN]);
        host.answer_next_prompt(Some(&["keep out"]));
        let mut plugin = FileMarks::default();
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        let before = notes("/project");

        hold(MAIN);
        let error = plugin.render(VIEW, None, &host.invocation()).unwrap_err();
        assert_eq!(error.to_string(), "can't check the mark on src/main.rs: the project would not answer");
        // Rebuilding the tree reports the same failure rather than drawing an
        // empty set and calling the marks gone.
        let error = FileMarks::default().activate(&Context::default()).unwrap_err();
        assert!(matches!(error, Error::Failed(_)), "{error:?}");
        assert!(host.file_notes().is_empty(), "nothing is drawn from a mark it could not check");
        assert_eq!(notes("/project"), before, "a stumble writes nothing over the notes");

        release(MAIN);
        show_flat(&mut plugin, &host);
        let view = plugin.render(VIEW, None, &host.invocation()).unwrap();
        assert_eq!(view.status.as_deref(), Some("1 mark"));
        assert_eq!(rows(&view).len(), 1, "the mark is back in the list once the project answers");
        FileMarks::default().activate(&Context::default()).unwrap();
        assert_eq!(remark(&host, MAIN).as_deref(), Some("keep out"), "so it goes back on the tree");
    }

    /// What a write just put in the file goes on the tree from what it wrote,
    /// not from reading the file back: a read answered for the older content
    /// would put every remark one operation behind, showing up only when the
    /// next one is written.
    #[test]
    fn a_new_remark_is_drawn_from_the_write_that_made_it() {
        let host = install();
        let notes = format!("/project/{FILE}");
        host.add_file(MAIN, "fn main() {}\n");
        host.select_paths(&[MAIN]);
        let mut plugin = FileMarks::default();

        answered_for(&notes, &text(&[]));
        host.answer_next_prompt(Some(&["the entry point"]));
        plugin.run_command("add-remark", &host.invocation(), None).unwrap();
        stop_answering_for(&notes);

        assert_eq!(remark(&host, MAIN).as_deref(), Some("the entry point"), "the name carries it at once");
        assert_eq!(kept("/project", "src/main.rs").as_deref(), Some("the entry point"));
    }

    /// The same the other way: a mark taken off leaves the tree on the pass
    /// that took it off, rather than waiting for the next write to notice.
    #[test]
    fn a_cleared_remark_leaves_the_tree_on_the_pass_that_cleared_it() {
        let host = install();
        let file = format!("/project/{FILE}");
        host.add_file(MAIN, "x");
        seed(&host, "/project", &[("src/main.rs", "keep out")]);
        let as_it_stands = notes("/project").unwrap();
        host.select_paths(&[MAIN]);
        let mut plugin = FileMarks::default();

        answered_for(&file, &as_it_stands);
        plugin.run_command("clear-mark", &host.invocation(), None).unwrap();
        stop_answering_for(&file);

        assert_eq!(remark(&host, MAIN), None, "an answer for the file before the write would redraw it");
        assert_eq!(kept("/project", "src/main.rs"), None);
    }

    /// The prune on a write is only for paths the project says are gone; one it
    /// cannot check keeps its note through the same write.
    #[test]
    fn a_write_keeps_what_the_project_cannot_check() {
        let host = install();
        host.add_file(MAIN, "x");
        host.add_file("/project/a.rs", "x");
        seed(&host, "/project", &[("src/main.rs", "keep out"), ("a.rs", "settled")]);

        hold(MAIN);
        host.select_paths(&["/project/a.rs"]);
        host.answer_next_prompt(Some(&["settled, still"]));
        // The write happens before the tree is redrawn, so what fails here is
        // the drawing, and the notes are already safe on disk.
        let error = FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::Failed(_)), "{error:?}");
        release(MAIN);

        assert_eq!(kept("/project", "src/main.rs").as_deref(), Some("keep out"), "unconfirmed is not deleted");
        assert_eq!(kept("/project", "a.rs").as_deref(), Some("settled, still"));
    }

    /// One file holds the notes now, so one read that fails is the whole set
    /// failing — and the one thing a write must never do is answer it.
    #[test]
    fn notes_that_cannot_be_read_are_never_written_over() {
        let host = install();
        host.add_file(MAIN, "x");
        seed(&host, "/project", &[("src/main.rs", "keep out")]);
        let before = notes("/project");
        let file = format!("/project/{FILE}");

        hold(&file);
        host.select_paths(&[MAIN]);
        // No answer is queued, so were the box to open the note would come back
        // as a cancelled prompt: the file is read, and the failure reported,
        // before anything is asked of the user.
        let error = FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::Failed(_)), "{error:?}");
        assert!(FileMarks::default().activate(&Context::default()).is_err(), "and the window says so too");

        release(&file);
        assert_eq!(notes("/project"), before, "every note is where it was");
        assert_eq!(kept("/project", "src/main.rs").as_deref(), Some("keep out"));
    }

    /// A file this plugin cannot make sense of is not a file to replace: it may
    /// be a newer one, or one the user was part way through editing.
    #[test]
    fn a_file_that_is_not_understood_is_refused_not_replaced() {
        let host = install();
        host.add_file(MAIN, "x");
        host.add_file(&format!("/project/{FILE}"), "{\n  \"version\": 2,\n  \"marks\": {\n    \"src/main.rs\": \"newer\"\n  }\n}\n");
        host.select_paths(&[MAIN]);
        let error = FileMarks::default().run_command("add-remark", &host.invocation(), None).unwrap_err();
        assert_eq!(error.to_string(), "invalid: file-mark.json: layout 2 is not one this plugin can read");
        assert!(notes("/project").unwrap().contains("\"version\": 2"), "the newer file is left for a newer plugin");

        host.add_file(&format!("/project/{FILE}"), "{ this is not the shape");
        let error = FileMarks::default().run_command("clear-mark", &host.invocation(), None).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
        assert_eq!(notes("/project").as_deref(), Some("{ this is not the shape"), "nor is a half-edited one");
    }

    /// Holds a project's read of `path` back, which is the one answer the mock
    /// has no way to give on its own.
    fn hold(path: &str) {
        HELD.with(|held| held.borrow_mut().insert(path.to_owned()));
    }

    fn release(path: &str) {
        HELD.with(|held| held.borrow_mut().remove(path));
    }

    /// A read of `path` answered with `text`, however the file stands now —
    /// which is what a read made in the same pass as a write can hand back.
    fn answered_for(path: &str, text: &str) {
        STALE.with(|stale| stale.borrow_mut().insert(path.to_owned(), text.to_owned()));
    }

    fn stop_answering_for(path: &str) {
        STALE.with(|stale| stale.borrow_mut().remove(path));
    }

    #[test]
    fn the_flat_list_puts_the_note_before_the_place() {
        let host = install();
        host.add_file(MAIN, "x");
        host.add_dir("/project/docs");
        let mut plugin = FileMarks::default();
        for (path, text) in [(MAIN, "keep out"), ("/project/docs", "hand me in")] {
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
        let host = install();
        host.add_file(MAIN, "x");
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        seed(&host, "/project", &[("src/main.rs", "mine")]);
        seed(&host, "/other", &[("b.rs", "theirs")]);
        let mut plugin = FileMarks::default();
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
        let host = install();
        host.add_file("/project/src/app/main.rs", "x");
        host.add_file("/project/src/lib.rs", "x");
        seed(&host, "/project", &[("src/app/main.rs", "the entry"), ("src/lib.rs", "mine")]);
        let mut plugin = FileMarks::default();

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
        let host = install();
        host.add_file(MAIN, "x");
        seed(&host, "/project", &[("src/main.rs", "keep out")]);
        let mut plugin = FileMarks::default();
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
        let host = install();
        host.add_file("/project/a.rs", "x");
        host.add_file("/project/b.rs", "x");
        seed(&host, "/project", &[("a.rs", "keep out"), ("b.rs", "settled")]);
        let mut plugin = FileMarks::default();
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
        let host = install();
        host.add_file("/project/a.rs", "x");
        seed(&host, "/project", &[("a.rs", "keep out")]);
        let mut plugin = FileMarks::default();

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
        let host = install();
        host.add_file("/project/a.rs", "x");
        seed(&host, "/project", &[("a.rs", "keep out")]);
        let mut plugin = FileMarks::default();
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
        let host = install();
        host.add_file(MAIN, "x");
        seed(&host, "/project", &[("src/main.rs", "keep out")]);
        let mut plugin = FileMarks::default();

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
        let host = install();
        host.add_file(MAIN, "x");
        host.add_dir("/project/docs");
        seed(&host, "/project", &[("docs", "keep out"), ("src/main.rs", "keep out")]);
        let mut plugin = FileMarks::default();

        plugin.handle(VIEW, ViewEvent::Activate(view_key(DIR, "/project/src")), &host.invocation()).unwrap();
        assert!(open_paths().is_empty(), "a folder shown for structure opens nothing");
        plugin.handle(VIEW, ViewEvent::Activate(view_key(MARK, "/project/docs")), &host.invocation()).unwrap();
        assert!(open_paths().is_empty(), "a marked folder has no editor either");

        plugin.handle(VIEW, ViewEvent::Activate(view_key(MARK, MAIN)), &host.invocation()).unwrap();
        assert_eq!(open_paths(), [MAIN]);
    }

    #[test]
    fn a_row_of_another_project_is_cleared_in_its_own() {
        let host = install();
        host.add_file("/other/b.rs", "x");
        host.add_project("/other");
        host.set_project_root(Some("/other"));
        seed(&host, "/other", &[("b.rs", "theirs")]);

        // The tool window lists every project, so a row names its own.
        let mut plugin = FileMarks::default();
        let out = plugin.run_command("clear-mark", &host.invocation(), Some(&view_key(MARK, "/other/b.rs"))).unwrap();
        assert_eq!(out.as_deref(), Some("Cleared 1 mark from b.rs."));
        assert_eq!(kept("/other", "b.rs"), None);
        assert_eq!(remark(&host, "/other/b.rs"), None);
    }

    #[test]
    fn another_view_or_command_is_not_ours() {
        let host = install();
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
