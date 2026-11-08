//! Links under the mouse: URLs and file paths in the output, and OSC 8 hyperlinks.

use super::*;

/// Where a link leads: a URL (browser) or a file (editor).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LinkTarget {
    Url(String),
    File(links::FileLink),
    /// A local path that isn't a plain file (a folder, an app bundle): shown in the file
    /// manager, since opening it could run it.
    Reveal(PathBuf),
}

/// What clicking `url` does. A `file:` URL never goes to the system as a URL, which would run
/// an app or reach a network share: a local file opens in the editor and any other local path
/// is revealed. Without `cwd` (over SSH) the path is the server's, so it's no link at all.
fn url_target(url: String, cwd: Option<&Path>) -> Option<LinkTarget> {
    let Some(rest) = url
        .get(5..)
        .filter(|_| url[..5].eq_ignore_ascii_case("file:"))
    else {
        return Some(LinkTarget::Url(url));
    };
    cwd?;
    // `file:///path` or `file://host/path`; `file:/path` too.
    let path = match rest.strip_prefix("//") {
        Some(authority) => &authority[authority.find('/')?..],
        None => rest.starts_with('/').then_some(rest)?,
    };
    let path = PathBuf::from(path);
    if path.is_file() {
        Some(LinkTarget::File(links::FileLink {
            path,
            line: None,
            column: None,
        }))
    } else {
        path.exists().then_some(LinkTarget::Reveal(path))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoveredLink {
    pub(super) target: LinkTarget,
    /// Cells (line, column) in grid coordinates.
    pub(super) cells: Vec<(i32, usize)>,
}

/// Link at `point`: the cell's OSC 8 hyperlink, or a URL written on the logical line
/// (rows wrapped by the terminal width are joined). Safe schemes only.
/// The link under `point`: an OSC 8 hyperlink, a written URL, or, with `cwd`, a path to an
/// existing file.
/// How many rows above and below the mouse a wrapped line is searched for links.
const LINK_SEARCH_ROWS: i32 = 32;

pub(super) fn link_at<T>(
    term: &Term<T>,
    point: AlacPoint,
    cwd: Option<&Path>,
) -> Option<HoveredLink> {
    let grid = term.grid();
    let last_column = alacritty_terminal::index::Column(term.columns() - 1);

    if let Some(hyperlink) = grid[point].hyperlink() {
        let url = hyperlink.uri().to_string();
        if !links::is_openable(&url) {
            return None;
        }
        let same = |column: usize| {
            grid[AlacPoint::new(point.line, alacritty_terminal::index::Column(column))]
                .hyperlink()
                .is_some_and(|h| h.id() == hyperlink.id())
        };
        let start = (0..=point.column.0)
            .rev()
            .take_while(|&c| same(c))
            .last()
            .unwrap_or(point.column.0);
        let end = (point.column.0..term.columns())
            .take_while(|&c| same(c))
            .last()
            .unwrap_or(point.column.0);
        let cells = (start..=end).map(|c| (point.line.0, c)).collect();
        // Local `file://` (`ls --hyperlink`, compilers): to the editor if it's a file.
        let target = url_target(url, cwd)?;
        return Some(HoveredLink { target, cells });
    }

    let wraps = |line: alacritty_terminal::index::Line| {
        grid[AlacPoint::new(line, last_column)]
            .flags
            .contains(Flags::WRAPLINE)
    };
    // Only the rows near the mouse: a minified file can be one wrapped line thousands of rows
    // long, and this runs on every move to another cell.
    let top = term.topmost_line().max(point.line - LINK_SEARCH_ROWS);
    let bottom = term.bottommost_line().min(point.line + LINK_SEARCH_ROWS);
    let mut first = point.line;
    while first > top && wraps(first - 1) {
        first -= 1;
    }
    let mut last = point.line;
    while last < bottom && wraps(last) {
        last += 1;
    }

    let mut text = String::new();
    let mut offsets: Vec<(usize, (i32, usize))> = Vec::new();
    let mut line = first;
    while line <= last {
        for column in 0..term.columns() {
            let cell = &grid[AlacPoint::new(line, alacritty_terminal::index::Column(column))];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            offsets.push((text.len(), (line.0, column)));
            text.push(cell.c);
        }
        line += 1;
    }

    // A wide character's spacer cell counts as the character.
    let hovered = offsets
        .iter()
        .rev()
        .find(|(_, (l, c))| *l == point.line.0 && *c <= point.column.0)
        .map(|(offset, _)| *offset)?;
    let url = links::find_urls(&text)
        .into_iter()
        .find(|r| r.contains(&hovered))
        .filter(|range| links::is_openable(&text[range.clone()]))
        .and_then(|range| {
            url_target(text[range.clone()].to_string(), cwd).map(|target| (range, target))
        });
    let file = || {
        let cwd = cwd?;
        let found = links::find_paths(&text)
            .into_iter()
            .find(|m| m.range.contains(&hovered))?;
        let home = crate::state::home_dir();
        let path = links::resolve_path(&found.path, cwd, home.as_deref())?;
        let file = links::FileLink {
            path,
            line: found.line,
            column: found.column,
        };
        Some((found.range, LinkTarget::File(file)))
    };
    let (range, target) = url.or_else(file)?;
    let cells = offsets
        .iter()
        .filter(|(offset, _)| range.contains(offset))
        .map(|(_, cell)| *cell)
        .collect();
    Some(HoveredLink { target, cells })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_paths_link_to_the_file_and_urls_win() {
        use alacritty_terminal::index::{Column, Line, Point};
        let cwd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let term = search_term(
            "  --> src/blocks.rs:12:5\r\nsee https://a.dev/src/x.rs",
            40,
            4,
        );
        let link = super::link_at(&term, Point::new(Line(0), Column(10)), Some(cwd)).unwrap();
        assert_eq!(
            link.target,
            super::LinkTarget::File(crate::links::FileLink {
                path: cwd.join("src/blocks.rs"),
                line: Some(12),
                column: Some(5),
            })
        );
        assert_eq!(link.cells.len(), "src/blocks.rs:12:5".len());
        // Without a folder (SSH) there are no file links; URLs still work.
        assert!(super::link_at(&term, Point::new(Line(0), Column(10)), None).is_none());
        let url = super::link_at(&term, Point::new(Line(1), Column(20)), Some(cwd)).unwrap();
        assert!(matches!(url.target, super::LinkTarget::Url(_)));
    }

    #[test]
    fn file_urls_never_go_to_the_system() {
        use super::{LinkTarget, url_target};
        let dir = std::env::temp_dir();
        let cwd = Some(dir.as_path());
        let file = dir.join(format!("moth-link-{}", std::process::id()));
        std::fs::write(&file, "").unwrap();
        let url = format!("FILE://host{}", file.display());
        assert!(matches!(
            url_target(url.clone(), cwd),
            Some(LinkTarget::File(_))
        ));
        // Over SSH, a server path.
        assert_eq!(url_target(url, None), None);
        let folder = format!("file://{}", dir.display());
        assert!(matches!(
            url_target(folder, cwd),
            Some(LinkTarget::Reveal(_))
        ));
        assert_eq!(url_target("file:///no/such/moth/path".into(), cwd), None);
        assert_eq!(url_target("file://server".into(), cwd), None);
        assert!(matches!(
            url_target("https://a.dev".into(), None),
            Some(LinkTarget::Url(_))
        ));
        std::fs::remove_file(file).ok();
    }
}
