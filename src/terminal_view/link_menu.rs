//! Right-click on a link in the output: what can be done with it. A file opens in the editor
//! or shows in the file manager, a folder opens in a new tab, and any of them can be copied.

use ui::ContextMenu;

use super::*;

/// What an entry does.
#[derive(Clone, Debug, PartialEq)]
enum LinkOp {
    Open(links::FileLink),
    OpenUrl(String),
    Reveal(PathBuf),
    OpenFolderInNewTab(PathBuf),
    Copy(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Entry(&'static str, LinkOp),
    Separator,
}

/// The file manager's name in "Show in …".
const SHOW_IN_FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else if cfg!(windows) {
    "Show in File Explorer"
} else {
    "Show in File Manager"
};

/// The menu for `target`. `is_dir` tells a folder from another path that isn't a plain file
/// (an app bundle is a folder too, but opening a tab in it is still fine).
fn items(target: &LinkTarget, is_dir: impl Fn(&Path) -> bool) -> Vec<Item> {
    let path_text = |path: &Path| path.display().to_string();
    match target {
        LinkTarget::Url(url) => vec![
            Item::Entry("Open Link", LinkOp::OpenUrl(url.clone())),
            Item::Separator,
            Item::Entry("Copy Link", LinkOp::Copy(url.clone())),
        ],
        LinkTarget::File(file) => {
            let mut items = vec![
                Item::Entry("Open", LinkOp::Open(file.clone())),
                Item::Entry(SHOW_IN_FILE_MANAGER, LinkOp::Reveal(file.path.clone())),
            ];
            if let Some(folder) = file.path.parent() {
                items.push(Item::Entry(
                    "Open Folder in New Tab",
                    LinkOp::OpenFolderInNewTab(folder.to_path_buf()),
                ));
            }
            items.push(Item::Separator);
            items.push(Item::Entry(
                "Copy Path",
                LinkOp::Copy(path_text(&file.path)),
            ));
            if let Some(line) = file.line {
                let position = match file.column {
                    Some(column) => format!("{}:{line}:{column}", path_text(&file.path)),
                    None => format!("{}:{line}", path_text(&file.path)),
                };
                items.push(Item::Entry("Copy Path and Line", LinkOp::Copy(position)));
            }
            items
        }
        LinkTarget::Reveal(path) => {
            let mut items = vec![Item::Entry(
                SHOW_IN_FILE_MANAGER,
                LinkOp::Reveal(path.clone()),
            )];
            if is_dir(path) {
                items.push(Item::Entry(
                    "Open in New Tab",
                    LinkOp::OpenFolderInNewTab(path.clone()),
                ));
            }
            items.push(Item::Separator);
            items.push(Item::Entry("Copy Path", LinkOp::Copy(path_text(path))));
            items
        }
    }
}

impl LinkOp {
    fn perform(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Open(file) => window.dispatch_action(
                Box::new(crate::workspace::OpenFile { file: file.clone() }),
                cx,
            ),
            Self::OpenUrl(url) => cx.open_url(url),
            Self::Reveal(path) => cx.reveal_path(path),
            Self::OpenFolderInNewTab(dir) => window.dispatch_action(
                Box::new(crate::workspace::OpenFolderInNewTab { dir: dir.clone() }),
                cx,
            ),
            Self::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text.clone())),
        }
    }
}

impl TerminalView {
    /// Opens the menu for `target` at `position`, with the keyboard on it; closing it gives the
    /// keyboard back to the terminal.
    pub(super) fn deploy_link_menu(
        &mut self,
        target: &LinkTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = items(target, Path::is_dir);
        let focus = self.focus_handle.clone();
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.context(focus);
            for item in items {
                menu = match item {
                    Item::Entry(label, op) => {
                        menu.entry(label, None, move |window, cx| op.perform(window, cx))
                    }
                    Item::Separator => menu.separator(),
                };
            }
            menu
        });
        menu.focus_handle(cx).focus(window, cx);
        let dismissed = cx.subscribe_in(
            &menu,
            window,
            |this, menu, _: &gpui::DismissEvent, window, cx| {
                if menu.focus_handle(cx).contains_focused(window, cx) {
                    this.focus_handle.focus(window, cx);
                }
                this.link_menu = None;
                cx.notify();
            },
        );
        self.link_menu = Some((menu, position, dismissed));
        cx.notify();
    }

    pub(super) fn render_link_menu(&self) -> Option<gpui::AnyElement> {
        let (menu, position, _) = self.link_menu.as_ref()?;
        Some(
            deferred(
                anchored()
                    .position(*position)
                    .anchor(gpui::Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu.clone()),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[Item]) -> Vec<&'static str> {
        items
            .iter()
            .map(|item| match item {
                Item::Entry(label, _) => *label,
                Item::Separator => "---",
            })
            .collect()
    }

    fn op<'a>(items: &'a [Item], label: &str) -> &'a LinkOp {
        items
            .iter()
            .find_map(|item| match item {
                Item::Entry(l, op) if *l == label => Some(op),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn a_file_opens_shows_its_folder_and_copies_its_position() {
        let file = links::FileLink {
            path: PathBuf::from("/repo/src/main.rs"),
            line: Some(12),
            column: Some(5),
        };
        let items = items(&LinkTarget::File(file.clone()), |_| false);
        assert_eq!(
            labels(&items),
            [
                "Open",
                SHOW_IN_FILE_MANAGER,
                "Open Folder in New Tab",
                "---",
                "Copy Path",
                "Copy Path and Line"
            ]
        );
        assert_eq!(op(&items, "Open"), &LinkOp::Open(file));
        assert_eq!(
            op(&items, "Open Folder in New Tab"),
            &LinkOp::OpenFolderInNewTab(PathBuf::from("/repo/src"))
        );
        let main = PathBuf::from("/repo/src/main.rs").display().to_string();
        assert_eq!(
            op(&items, "Copy Path and Line"),
            &LinkOp::Copy(format!("{main}:12:5"))
        );
    }

    #[test]
    fn a_file_without_a_line_has_no_position_to_copy() {
        let file = links::FileLink {
            path: PathBuf::from("/repo/README.md"),
            line: None,
            column: None,
        };
        let items = items(&LinkTarget::File(file), |_| false);
        assert!(!labels(&items).contains(&"Copy Path and Line"));
    }

    #[test]
    fn a_folder_opens_in_a_new_tab_and_a_url_in_the_browser() {
        let folder = PathBuf::from("/repo/target");
        let items_for_folder = items(&LinkTarget::Reveal(folder.clone()), |_| true);
        assert_eq!(
            labels(&items_for_folder),
            [SHOW_IN_FILE_MANAGER, "Open in New Tab", "---", "Copy Path"]
        );
        assert_eq!(
            op(&items_for_folder, "Open in New Tab"),
            &LinkOp::OpenFolderInNewTab(folder.clone())
        );
        // Something that exists but isn't a folder (a socket, say): only shown and copied.
        let other = items(&LinkTarget::Reveal(folder), |_| false);
        assert_eq!(labels(&other), [SHOW_IN_FILE_MANAGER, "---", "Copy Path"]);
        let url = items(&LinkTarget::Url("https://a.dev/x".into()), |_| false);
        assert_eq!(labels(&url), ["Open Link", "---", "Copy Link"]);
        assert_eq!(
            op(&url, "Copy Link"),
            &LinkOp::Copy("https://a.dev/x".into())
        );
    }
}
