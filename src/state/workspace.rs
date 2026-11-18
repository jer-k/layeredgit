//! Repository selection, cached review documents and ref-picker actions.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use repo::Branch;
use rune::{FileDialogOptions, Key, KeyEvent, PointerPress, Tx};
use rune_kit::ComboboxState;

use crate::{
    APP_NAME,
    cli::Launch,
    model::{Review, fresh::Replacement},
    preferences::Preferences,
    repositories::{
        Choice, Discovered, Entry, Request, Side, Summary, Visibility, Workspace, discover, row_key,
    },
    source::{Repository, ref_label},
};

use super::{State, Status};

impl State {
    pub(crate) fn with_workspace(workspace: Workspace) -> Self {
        let mut state = Self::new(Review::default(), Status::Empty, None);
        state.workspace = Some(workspace);
        state
    }

    pub(crate) fn preview_workspace(
        preload: Vec<PathBuf>,
        launch: Launch,
        demo_pulse: bool,
    ) -> Self {
        let workspace = Workspace {
            demo_pulse,
            ..Workspace::default()
        };
        let mut state = Self::with_workspace(workspace);
        let mut tx = Tx::default();
        for path in preload {
            match discover(Repository { path, base: None }) {
                Ok(discovered) => {
                    state.insert_repository(discovered);
                }
                Err(error) => state.workspace_error(error),
            }
        }
        if let Launch::Review(repository) = launch {
            state.repository_resolved(discover(repository), &mut tx);
        }
        state
    }

    pub(crate) fn restore_workspace(&mut self, prefs: Preferences, select: bool, tx: &mut Tx) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        workspace.collapsed = prefs.collapsed;
        workspace.entries = prefs
            .repositories
            .into_iter()
            .map(|(path, base)| Entry::new(Repository { path, base }))
            .collect();
        if select {
            let index = prefs.selected.and_then(|path| {
                workspace
                    .entries
                    .iter()
                    .position(|entry| entry.repository.path == path)
            });
            self.select_repository(index.unwrap_or(0), tx);
        }
    }

    pub(crate) fn workspace_error(&mut self, error: String) {
        if let Some(workspace) = &mut self.workspace {
            workspace.add_error = Some(error);
        }
    }

    pub(crate) fn pick_folder(&mut self, tx: &mut Tx) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        if workspace.pending {
            return;
        }
        workspace.add_error = None;
        tx.open_folder(
            FileDialogOptions {
                title: "Add repository".into(),
                ..FileDialogOptions::default()
            },
            |state: &mut Self, answer, tx| match answer {
                Ok(Some(path)) => {
                    let repository = Repository { path, base: None };
                    if let Some(workspace) = &mut state.workspace {
                        if workspace.sender.is_none() {
                            state.repository_resolved(discover(repository), tx);
                            return;
                        }
                        workspace.pending =
                            workspace.send(Request::Launch(Launch::Review(repository)));
                        if !workspace.pending {
                            state.workspace_error("Couldn’t reach the repository worker.".into());
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => state.workspace_error(error.to_string()),
            },
        );
    }

    pub(crate) fn repository_resolved(&mut self, result: Result<Discovered, String>, tx: &mut Tx) {
        if let Some(workspace) = &mut self.workspace {
            workspace.pending = false;
            workspace.add_error = None;
        }
        match result {
            Ok(mut found) => {
                let base = found.repository.base.take();
                if let Some(index) = self.insert_repository(found) {
                    self.select_repository(index, tx);
                    if let Some(base) = base {
                        self.choose_revision(Side::Base, Some(base), tx);
                    }
                }
            }
            Err(error) => {
                if self
                    .workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.selected.is_none())
                {
                    self.select_repository(0, tx);
                }
                self.workspace_error(error);
            }
        }
        tx.instant();
    }

    fn insert_repository(&mut self, found: Discovered) -> Option<usize> {
        let workspace = self.workspace.as_mut()?;
        let index = if let Some(index) = workspace
            .entries
            .iter()
            .position(|entry| entry.repository.path == found.repository.path)
        {
            index
        } else {
            let index = workspace.entries.len();
            workspace.entries.push(Entry::new(found.repository));
            index
        };
        let entry = workspace.entries.get_mut(index)?;
        entry.summary = found.summary;
        entry.branches = found.branches;
        Some(index)
    }

    pub(crate) fn update_summary(
        &mut self,
        path: &Path,
        summary: Summary,
        branches: Option<Vec<Branch>>,
    ) -> bool {
        let Some(entry) = self.workspace.as_mut().and_then(|workspace| {
            workspace
                .entries
                .iter_mut()
                .find(|entry| entry.repository.path == path)
        }) else {
            return false;
        };
        let mut changed = entry.summary != summary;
        entry.summary = summary;
        if let Some(branches) = branches {
            changed |= entry.branches != branches;
            entry.branches = branches;
        }
        changed
    }

    pub(super) fn set_checkout_dirty(&mut self, dirty: bool) -> bool {
        let Some(entry) = self
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.entries.get_mut(workspace.selected?))
        else {
            return false;
        };
        std::mem::replace(&mut entry.summary.dirty, dirty) != dirty
    }

    pub(crate) fn edit_menu(&mut self, path: PathBuf, point: PointerPress) {
        if let Some(workspace) = &mut self.workspace {
            workspace.menu = Some((path, point));
        }
    }

    pub(crate) fn close_menu(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            workspace.menu = None;
        }
    }

    pub(crate) fn remove_repository(&mut self, path: &Path, tx: &mut Tx) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        workspace.menu = None;
        let Some(index) = workspace
            .entries
            .iter()
            .position(|entry| entry.repository.path == path)
        else {
            return;
        };
        let selected = workspace.selected;
        workspace.entries.remove(index);
        match selected {
            Some(active) if active == index => {
                let next = workspace
                    .entries
                    .len()
                    .checked_sub(1)
                    .map(|last| index.min(last));
                workspace.selected = None;
                self.swap_repository(next, tx);
            }
            Some(active) if active > index => workspace.selected = Some(active - 1),
            _ => {}
        }
        self.save_workspace();
    }

    pub(crate) fn toggle_repositories(&mut self, tx: &mut Tx) {
        if let Some(workspace) = &mut self.workspace {
            workspace.collapsed = !workspace.collapsed;
            tx.instant();
            self.save_workspace();
        }
    }

    pub(crate) fn select_repository(&mut self, index: usize, tx: &mut Tx) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if workspace.selected == Some(index) || workspace.entries.get(index).is_none() {
            return;
        }
        self.swap_repository(Some(index), tx);
        self.save_workspace();
    }

    pub(crate) fn select_repository_path(&mut self, path: &Path, tx: &mut Tx) {
        let index = self.workspace.as_ref().and_then(|workspace| {
            workspace
                .entries
                .iter()
                .position(|entry| entry.repository.path == path)
        });
        if let Some(index) = index {
            self.select_repository(index, tx);
        }
    }

    fn swap_repository(&mut self, index: Option<usize>, tx: &mut Tx) {
        let Some(mut workspace) = self.workspace.take() else {
            return;
        };
        if let Some(refresh) = &mut self.refresh {
            refresh.suspend();
        }
        self.pulse.clear();
        workspace.menu = None;
        for picker in &mut workspace.picker {
            picker.close();
        }
        workspace.observed_picker = None;
        let previous = std::mem::replace(&mut workspace.selected, index);
        let request = index
            .and_then(|index| workspace.entries.get(index))
            .map(|entry| Request::Inspect(entry.repository.path.clone()));
        let live = workspace.sender.is_some();
        let mut next = index
            .and_then(|index| workspace.entries.get_mut(index))
            .map_or_else(
                || Self::new(Review::default(), Status::Empty, None),
                |entry| {
                    let mut document = entry.document.take().map_or_else(
                        || Self::open(Some(entry.repository.clone()), !live, workspace.demo_pulse),
                        |document| *document,
                    );
                    if std::mem::take(&mut entry.reload) {
                        document.retarget_review(entry.repository.clone(), entry.head.clone(), tx);
                    }
                    document
                },
            );
        next.rows_version = self.rows_version.wrapping_add(1);
        next.review_version = self.review_version.wrapping_add(1);
        next.headless_reduced_motion = self.headless_reduced_motion;
        #[cfg(feature = "dev")]
        std::mem::swap(&mut next.tuning, &mut self.tuning);
        if let Some(refresh) = &mut next.refresh {
            if let Some(waker) = &workspace.waker {
                refresh.setup(Arc::clone(waker));
            }
            refresh.visibility(matches!(workspace.window, Visibility::Shown), tx);
            refresh.activate(tx);
            if matches!(next.status, Status::Changing) {
                refresh.force(tx);
            }
        }
        next.sidebar_reveal = next.active_file;
        let [left, top] = next.scroll_position.get();
        tx.scroll_to_instant("diff-scroll", left, top);
        if let Some(entry) = index.and_then(|index| workspace.entries.get(index)) {
            tx.scroll_into_view_in("repository-list", row_key(&entry.repository.path));
        }
        let old = std::mem::replace(self, next);
        if let Some(entry) = previous.and_then(|index| workspace.entries.get_mut(index)) {
            entry.document = Some(Box::new(old));
        }
        if let Some(request) = request {
            workspace.send(request);
        }
        self.workspace = Some(workspace);
        self.title(tx);
        tx.instant();
    }

    fn save_workspace(&self) {
        if let Some(workspace) = &self.workspace {
            workspace.send(Request::Save(workspace.preferences()));
        }
    }

    pub(super) fn title(&self, tx: &mut Tx) {
        if let Some(entry) = self.workspace.as_ref().and_then(Workspace::active) {
            tx.set_title(format!(
                "{} — {}…{}",
                entry.name,
                self.active_label(Side::Base),
                self.active_label(Side::Head)
            ));
        } else if self.workspace.is_some() {
            tx.set_title(APP_NAME);
        }
    }

    pub(crate) fn ref_name(&self, side: Side) -> &str {
        match side {
            Side::Base => &self.review.base_label,
            Side::Head => &self.review.head_label,
        }
    }

    pub(crate) fn picker_mut(&mut self, side: Side) -> &mut ComboboxState {
        let workspace = self.workspace.get_or_insert_with(Workspace::default);
        let [base, head] = &mut workspace.picker;
        match side {
            Side::Base => base,
            Side::Head => head,
        }
    }

    pub(super) fn inspect_open_picker(&mut self) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let [base, head] = &workspace.picker;
        let open = if base.is_open() {
            Some(Side::Base)
        } else if head.is_open() {
            Some(Side::Head)
        } else {
            None
        };
        if std::mem::replace(&mut workspace.observed_picker, open) != open
            && open.is_some()
            && let Some(entry) = workspace.active()
        {
            workspace.send(Request::Inspect(entry.repository.path.clone()));
        }
    }

    pub(crate) fn choose_revision(&mut self, side: Side, reference: Option<OsString>, tx: &mut Tx) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let Some(entry) = workspace
            .selected
            .and_then(|index| workspace.entries.get_mut(index))
        else {
            return;
        };
        let mut repository = entry.repository.clone();
        let mut head = entry.head.clone();
        match side {
            Side::Base => repository.base = reference,
            Side::Head => head = reference,
        }
        if entry.repository == repository && entry.head == head && entry.validating.is_none() {
            entry.revision_error = None;
            return;
        }
        workspace.serial = workspace.serial.wrapping_add(1);
        let choice = Choice {
            id: workspace.serial,
            side,
            repository,
            head,
        };
        entry.revision_error = None;
        entry.validating = Some(choice.id);
        if workspace.sender.is_none() {
            let input = choice
                .repository
                .preview(choice.head.as_deref())
                .map_err(|error| error.to_string());
            let result = input.as_ref().map(|_| ()).map_err(Clone::clone);
            self.finish_choice(choice, result, tx);
            if let Ok((input, worktree)) = input {
                self.set_worktree(worktree);
                let mut update = Replacement::new(Review::new(input), &self.review);
                update.fresh.clear();
                self.replace_review(update, tx);
            }
        } else if !workspace.send(Request::Choose(choice.clone())) {
            self.finish_choice(
                choice,
                Err("Couldn’t reach the repository worker.".into()),
                tx,
            );
        }
    }

    pub(crate) fn finish_choice(
        &mut self,
        choice: Choice,
        result: Result<(), String>,
        tx: &mut Tx,
    ) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let Some((index, entry)) = workspace.entries.iter_mut().enumerate().find(|(_, entry)| {
            entry.repository.path == choice.repository.path && entry.validating == Some(choice.id)
        }) else {
            return;
        };
        entry.validating = None;
        match result {
            Ok(()) => {
                entry.repository = choice.repository.clone();
                entry.head.clone_from(&choice.head);
                if workspace.selected == Some(index) {
                    self.retarget_review(choice.repository, choice.head, tx);
                } else {
                    entry.reload = true;
                }
                self.save_workspace();
            }
            Err(error) => {
                entry.revision_error = Some(format!("{}: {error}", choice.side.label()));
            }
        }
    }

    fn retarget_review(&mut self, repository: Repository, head: Option<OsString>, tx: &mut Tx) {
        self.pulse.clear();
        if let Some(refresh) = &mut self.refresh {
            if matches!(self.status, Status::Ready { .. }) {
                self.status = Status::Changing;
            }
            refresh.retarget(repository, head, tx);
        }
    }

    pub(crate) fn active_label(&self, side: Side) -> String {
        let displayed = self.ref_name(side);
        if !displayed.is_empty() && matches!(self.status, Status::Ready { .. }) {
            return displayed.into();
        }
        self.workspace
            .as_ref()
            .and_then(Workspace::active)
            .map_or_else(
                || displayed.into(),
                |entry| match side {
                    Side::Base => entry
                        .repository
                        .base
                        .as_deref()
                        .map_or_else(|| "Default".into(), ref_label),
                    Side::Head => entry.head.as_deref().map_or_else(
                        || {
                            if entry.summary.branch.is_empty() {
                                "Current".into()
                            } else {
                                entry.summary.branch.clone()
                            }
                        },
                        ref_label,
                    ),
                },
            )
    }

    pub(super) fn workspace_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(workspace) = &self.workspace else {
            return false;
        };
        if !event.mods.cmd || event.mods.ctrl || event.mods.alt || event.mods.shift {
            return false;
        }
        let character = match &event.key {
            Key::Command(character) => Some(*character),
            Key::Text(key) if key.len() == 1 => key.chars().next(),
            _ => None,
        };
        if matches!(character, Some('o' | 'O')) {
            self.pick_folder(tx);
            return true;
        }
        let count = workspace.entries.len();
        if count == 0 {
            return false;
        }
        let current = workspace.selected.unwrap_or(0);
        let index = match character {
            Some('[') => {
                if current == 0 {
                    count - 1
                } else {
                    current - 1
                }
            }
            Some(']') => (current + 1) % count,
            Some(number @ '1'..='9') => number as usize - '1' as usize,
            _ => return false,
        };
        self.select_repository(index, tx);
        true
    }
}
