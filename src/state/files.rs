//! Sidebar tree ordered exactly like the diff, with filter-aware paths and badges.

use std::{collections::HashSet, path::Path};

use rune::{KeyEvent, Tx};
use rune_kit::{FileNode, TreeKey, TreeModel};

use crate::{
    input::ChangeKind,
    model::{Filter, Review},
};

pub(crate) type FileTree = TreeModel<FileNode<Badge>>;

pub(crate) struct Badge {
    pub(crate) file: usize,
    pub(crate) kind: ChangeKind,
    pub(crate) working: bool,
}

pub(crate) struct Sidebar {
    pub(crate) tree: FileTree,
    folders: Vec<String>,
    closed: HashSet<String>,
    ids: Vec<Option<String>>,
}

impl Sidebar {
    pub(super) fn new(review: &Review, filter: Filter) -> Self {
        let mut sidebar = Self {
            tree: TreeModel::new().key("file-tree"),
            folders: Vec::new(),
            closed: HashSet::new(),
            ids: Vec::new(),
        };
        sidebar.rebuild(review, filter);
        sidebar
    }

    pub(super) fn rebuild(&mut self, review: &Review, filter: Filter) {
        for folder in self.folders.drain(..) {
            if self.tree.is_expanded(&folder) {
                self.closed.remove(&folder);
            } else {
                self.closed.insert(folder);
            }
        }
        self.tree.clear();
        self.ids = vec![None; review.files.len()];
        for &index in review.file_order(filter) {
            let Some(file) = review.files.get(index) else {
                continue;
            };
            let path = file.display_path(filter);
            let ancestors: Vec<_> = path
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
                .collect();
            let mut parent = None;
            for folder in ancestors.into_iter().rev() {
                let id = path_id("folder", folder);
                if !self.tree.contains(&id) {
                    self.tree
                        .insert(parent.as_deref(), id.clone(), node(folder, None));
                    self.folders.push(id.clone());
                }
                parent = Some(id);
            }
            let mut id = path_id("file", path);
            if self.tree.contains(&id) {
                id = format!("file-duplicate/{index}/{id}");
            }
            let (kind, working) = file.badge(filter);
            self.tree.insert(
                parent.as_deref(),
                id.clone(),
                node(
                    path,
                    Some(Badge {
                        file: index,
                        kind,
                        working,
                    }),
                ),
            );
            if let Some(slot) = self.ids.get_mut(index) {
                *slot = Some(id);
            }
        }
        self.tree.expand_all();
        for folder in &self.closed {
            self.tree.set_expanded(folder, false);
        }
    }

    pub(super) fn select(&mut self, file: usize) {
        if let Some(id) = self.ids.get(file).and_then(Option::as_deref)
            && self.tree.visible_row(id).is_some()
        {
            self.tree.select(id);
        }
    }

    pub(super) fn reveal(&self, file: usize, tx: &mut Tx) {
        if let Some(id) = self.ids.get(file).and_then(Option::as_deref)
            && self.tree.visible_row(id).is_some()
        {
            tx.scroll_into_view_in("file-tree/viewport", id);
        }
    }

    pub(super) fn key(&mut self, event: &KeyEvent, tx: &mut Tx) -> (bool, Option<usize>) {
        let before = self.tree.cursor().map(str::to_owned);
        match self.tree.handle_key(event, FileNode::label, tx) {
            TreeKey::Ignored => (false, None),
            TreeKey::Open(id) => (true, self.file(&id)),
            TreeKey::Handled => {
                let file = self
                    .tree
                    .cursor()
                    .filter(|&id| Some(id) != before.as_deref())
                    .and_then(|id| self.file(id));
                (true, file)
            }
        }
    }

    pub(super) fn file(&self, id: &str) -> Option<usize> {
        self.tree.get(id)?.data.as_ref().map(|badge| badge.file)
    }
}

fn node(path: &Path, data: Option<Badge>) -> FileNode<Badge> {
    FileNode {
        name: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        path: path.to_string_lossy().into_owned(),
        dir: data.is_none(),
        data,
    }
}

fn path_id(prefix: &str, path: &Path) -> String {
    if let Some(name) = path.to_str() {
        format!("{prefix}/{name}")
    } else {
        format!(
            "{prefix}-bytes/{:02x?}",
            path.as_os_str().as_encoded_bytes()
        )
    }
}
