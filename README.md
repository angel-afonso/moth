# Moth

A macOS terminal built with [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) and the components from Zed's `ui` crate. It uses the alacritty emulator (the same fork Zed uses).

The name comes from the moth the Harvard Mark II team found stuck in a relay in 1947 and taped into the logbook as the "first actual case of bug being found".

```bash
cargo run
```

Requires Rust 1.99 (pinned in `rust-toolchain.toml`). Metal shaders are compiled at startup (`runtime_shaders`), so Xcode's Metal Toolchain isn't needed. For release builds it's worth installing it (`xcodebuild -downloadComponent MetalToolchain`) and dropping that feature.

On launch, the app restores the windows, tabs and panes from last time (see "Windows and session"); on first run, or with no saved session, it opens in the last folder you used (stored in `~/Library/Application Support/moth/last_cwd`). Each new tab opens in the active tab's folder. The shell is launched directly as a login shell (`$SHELL -l`), without going through `/usr/bin/login`, so there's no "Last login: …" line.

## Shortcuts

| Shortcut | Action |
| --- | --- |
| ⌘N | New window (in the active pane's folder) |
| ⌘T | New tab |
| ⌘M | Minimize the window |
| ⌘W | Close the pane (or the tab if it only has one) |
| Middle-click on a tab | Close the tab |
| ⌘D / ⌘⇧D | Split the pane right / down |
| ⌘⌥← ↑ → ↓ · ⌘[ / ⌘] | Go to the pane on that side · previous / next pane |
| ⌘⇧⏎ | Maximize or restore the pane |
| ⌘⇧T | Reopen the last closed tab in its folder |
| ⌘+ / ⌘− / ⌘0 | Increase / decrease / reset the text size |
| ⌘F | Find in the output and scrollback |
| ⌘↑ / ⌘↓ | Go to the previous / next command |
| ⌘⇧C | Copy the output of the selected command, or of the last one |
| ⌘1…⌘8, ⌘9 | Go to tab N / to the last tab |
| ⌘⇧] / ⌘⇧[ · ⌃Tab / ⌃⇧Tab | Next / previous tab |
| ⌘K · ⌘⇧P | Command palette |
| ⌘R | Search history (⏎ puts it on the prompt, ⌘⏎ runs it) |
| ⌘L | Clear the screen and scrollback |
| ⌘⇧R | Reconnect a dropped SSH session |
| ⌘V | Paste (honors bracketed paste) |
| ⌘C | Copy the selection (with no selection, the visible screen) |
| Tab | Shell completion, as usual (`Doc` → `Documents/`) |
| ⌃Space | App suggestions (↑↓ choose, Tab complete, ⏎ run, esc close) |
| → / End | Accept the history suggestion |
| ⌥← / ⌥→ · ⌥⌦ | Previous / next word · delete the next word (at the prompt) |
| ⌘← / ⌘→ · ⌘⌫ | Start / end of line · delete the line (at the prompt) |

## Panes

⌘D splits the active pane to the right and ⌘⇧D splits it down; the new pane opens in the same folder and gets focus. Splitting halves the pane; splitting several times in the same direction adds panes side by side instead of nesting them. Drag the divider between panes to resize them (no pane goes below 80 px), and when a pane is closed its space goes to its neighbor. Clicking a pane focuses it, and the others are dimmed. ⌘⌥ plus the arrow keys moves to the pane on that side, and ⌘[ / ⌘] cycle through panes in reading order. ⌘⇧⏎ maximizes the active pane within the tab and restores it.

⌘W closes the active pane (asking for confirmation if something is running) and focus moves to the previous one; with a single pane it closes the tab. The tab's × closes it along with all its panes. The tab title and the status bar reflect the active pane.

## Windows and session

⌘N opens another window, offset slightly from the current one and in the active pane's folder. ⌘Q asks for confirmation if anything is running in any window. Besides Shell, the menu bar has Edit (copy, paste, find, copy command output, clear) and Window (minimize, zoom, tabs and panes).

The app saves the windows (with their position), their tabs and the pane layout with each pane's folder to `session.json`, and reopens them the same way on relaunch. It doesn't restore processes, screen contents or SSH sessions. Quitting with ⌘Q keeps everything; closing a window by hand discards it, and if it was the last one, the next launch starts fresh. To never restore anything, set `"restore_session": false` in the settings.

## Keyboard

At the prompt, ⌥ and ⌘ edit like everywhere else in macOS (they send `ESC b`/`ESC f` and `⌃A`/`⌃E`/`⌃U`, which zsh, bash and readline understand). In full-screen programs (vim, less, htop) those keys send the standard xterm sequences.

By default ⌥+letter types the macOS character, because on layouts such as Spanish ⌥ is how you type `@`, `#` or `|`. "Use ⌥ as Meta" in the palette (⌘K) makes ⌥+key send `ESC` + key (⌥B, ⌥F, ⌥. in zsh); the choice persists across runs.

## Closing tabs and quitting

If a tab has a running command or an SSH session, ⌘W, ⌘Q, the tab's close button or the window's close button ask for confirmation ("Close this tab?", "Close this pane?", "Quit Moth?") and list what would be interrupted (⏎ confirms, esc cancels; pressing the shortcut again also confirms). This needs the zsh integration to know what's running.

When the shell exits with `exit` or ⌃D, the tab closes. If it's killed by a signal or exits with an error right after starting (a broken `.zshrc`, for example), the tab stays open with the output and a banner: "Open New Shell" (⏎) starts a new shell in the same folder and "Close" (⌘W) closes the tab.

## Notifications, zoom and reopening tabs

When a command runs for at least 10 seconds and finishes in an inactive tab or while the window is in the background, the app flags the tab and shows a notice with the command, its result and duration. The notice is dismissed with its button. If the app is in the background, a macOS notification also appears with the result (✓ or ✕), the command and the duration; `"notifications": false` in the settings turns it off. Requires the zsh integration.

Zoom changes the text in every tab, adjusts the cell size and persists across runs. The range is 8 to 32 points and ⌘0 resets it to 13.5 points.

⌘⇧T ("Reopen Closed Tab") brings back up to the last 20 folders of closed tabs, even after restarting the app. It opens a new shell: it doesn't restore processes or re-run SSH commands. Folders that no longer exist are skipped.

## Command blocks

With the zsh integration, the output is split into command blocks: a prompt, the command and what it printed.

- ⌘↑ and ⌘↓ ("Go to Previous Command") step through commands: the selected one is highlighted with an accent bar and scrolled to the top of the view. Past the last one, ⌘↓ returns to the bottom; typing anything clears the selection.
- ⌘⇧C copies the output of the selected command or, with nothing selected, of the last command ("Copy Last Command Output"). It copies only the output, without the prompt or the command.
- Commands that failed (including ⌃C) get a red bar in the left margin.

To delimit blocks, the PTY scanner inserts a few invisible zero-width characters after each end of prompt (`OSC 133;B`), which alacritty stores in the cell. That way blocks follow the text when scrolling, when the window is resized or when the scrollback drops old lines. These characters are stripped when copying and never drawn. The integration also reports how many rows the prompt takes up; with themes that build the prompt at runtime (p10k), that number can come up short and the block may include the first row of the next prompt.

## Find in output

⌘F ("Find in Output") opens a field above the terminal. It searches for literal text in the screen and scrollback, including wrapped lines; it highlights the match and scrolls the view to it. ⏎ finds the next match, ⇧⏎ the previous one and Esc closes the find bar. The ↑ and ↓ buttons also navigate, and the search wraps around at either end. Lowercase queries are case-insensitive; including an uppercase letter makes the search case-sensitive.

## Selection and mouse

Dragging selects, double-click selects a word, triple-click a line and Shift+click extends the selection; ⌘C copies it and typing clears it. When a program requests the mouse (vim, htop, lazygit, Claude Code…), clicks, drags and the scroll wheel go to it in SGR or X10 format; hold Shift to select anyway.

## Completions

Tab at the prompt is the shell's completion, so navigating folders with `Doc` + Tab works like in any terminal. The app's suggestions are separate so they don't get in the way of that habit: the history suggestion appears in gray and is accepted with → or End, and ⌃Space ("Show Suggestions") opens a popup with what can be typed at the cursor: commands (PATH, aliases, functions, builtins), subcommands and flags of common tools with their descriptions, git branches, `package.json` scripts, hosts from `~/.ssh/config`, paths and history. The rest of the selected candidate is shown in gray on the line, and a side panel shows its documentation (usage and flags). If there's only one candidate it's completed right away, and after completing a folder the popup reopens with its contents. With nothing to suggest, ⌃Space goes to the shell as usual. With `"tab_completion": "popup"` in the settings, Tab opens the popup as it used to. The specs live in [`assets/completions/specs.json`](assets/completions/specs.json) and can be extended.

## Links

URLs in the output (`http`, `https`, `ftp`, `file`, `mailto`), including ones wrapped across two rows, and OSC 8 hyperlinks (`ls --hyperlink`, compilers) are underlined on hover and open in the default browser with a click. They open when the button is released over the same link, and a click that only activates the window opens nothing. On the line you're typing, a click still moves the cursor. For safety, other schemes are never opened, even when they arrive via OSC 8.

File paths are links too, with their line and column if the output includes them: `src/main.rs:12:5` (rustc, grep, go, pytest), `src/app.ts(12,5)` (tsc) and `File "app.py", line 7` (Python). They're resolved against the tab's current folder (also `~/…`, absolute paths and the `a/…` and `b/…` paths from `git diff`), and are only underlined if the file exists. A click opens them in the editor:

- the one set as `editor` in the settings, or the first one found among Zed, Cursor, VS Code, Windsurf and Sublime Text (on the shell's PATH or inside the app in `/Applications`);
- if none is found, the macOS default app, without jumping to the line.

Terminal editors (vim, nvim, helix, nano, micro, kak) open in a new tab. In an SSH session paths aren't links, since they're files on the server. Local `file://` hyperlinks also go to the editor.

## Settings

"Open Settings (font, editor, scrollback)" in the palette (⌘K) creates and opens `~/Library/Application Support/moth/config.json`. Everything is optional:

```json
{
  "font_family": "JetBrainsMono Nerd Font",
  "editor": "zed",
  "scrollback": 10000,
  "restore_session": true,
  "notifications": true,
  "tab_completion": "shell"
}
```

- `font_family`: the terminal font. With Geist Mono (the default), starship or p10k icons come from "Symbols Nerd Font" if you have it installed.
- `editor`: `zed`, `code`, `cursor`, `windsurf`, `subl`, `idea`, `nvim`, `hx`… or a template with `{file}`, `{line}` and `{column}`, such as `"myeditor --goto {file}:{line}"`.
- `scrollback`: lines kept by each new tab (up to 100,000).
- `restore_session`: reopen the previous session's windows, tabs and panes (on by default).
- `notifications`: macOS notification when a long command finishes while the app is in the background (on by default).
- `tab_completion`: `"shell"` (default) leaves Tab to zsh's completion; `"popup"` makes Tab open the app's suggestions.

The file is reloaded when the window regains focus. A misspelled key or invalid JSON shows a notice and the defaults are used.

## Command palette and history

⌘K opens the command palette: actions (with their shortcuts), open tabs and recent folders, which open in a new tab. ⌘R ("Search History") opens the zsh history search: it starts with whatever was already typed at the prompt, filters with fuzzy matching and highlights the matching characters. ⏎ puts the command on the prompt for editing and ⌘⏎ runs it. Clicking the branch in the status bar (or "Switch Branch…" in the palette) opens the same picker with the local branches.

## SSH sessions

When you run `ssh` (with the zsh integration), the app recognizes the destination (`user@host`, `-p`, `-J`, `-l`, `ssh://…`) and resolves it with `ssh -G`, which applies your `~/.ssh/config` without connecting. For the duration of the session:

- the tab shows the host with a ⇄ icon, its chip (`PROD`, `STG`…) and the top line in the host's color;
- the status bar switches to `user@host`, `hostname:port`, the remote folder (if the server puts it in the title, as Debian and Ubuntu do) and the time connected;
- if the host is production, a banner appears below the tabs, a border surrounds the terminal, and pasting multiple lines asks for confirmation (esc cancels, ⌘⏎ is "Paste Anyway").

If `ssh` exits with code 255 (the connection dropped), the tab is struck through and gets a button to reconnect (also ⌘⇧R or "Reconnect" in the palette). "Connect to…" in ⌘K lists dropped sessions, hosts from `~/.ssh/config` (following its `Include`s) and `ssh` commands from history; ⏎ opens a new tab and ⌘⏎ connects in the current one.

Each host's identity is defined in `~/Library/Application Support/moth/hosts.json`. Each rule is matched (glob with `*` and `?`) against the alias and against the real hostname; the first match wins:

```json
{
  "hosts": [
    { "match": "prod-*", "label": "PROD", "color": "red", "production": true },
    { "match": "*.staging.internal", "label": "STG", "color": "cyan" }
  ]
}
```

`color` accepts ANSI colors from the theme (`red`, `green`, `yellow`, `blue`, `magenta`, `cyan`) or a hex value. Without that file, built-in rules are used: `*prod*`, `*production*` and `*live*` as production, and `*staging*` and `*stg*` as STG. If you create the file, it replaces those rules. Hosts without a rule get a stable color based on their name.

## Themes

Colors come from tokens (the ones on the design's "Tokens and components" sheet): UI, status, command-line syntax and the 16 ANSI colors. Two themes ship with the app, **Graphite** (dark) and **Graphite Light**. "Change Theme…" in the palette (⌘K) opens the picker: ↑↓ shows a live preview, ⏎ saves it and esc goes back to the previous one. Switching also recolors the output already on screen and the prompt, since they use ANSI colors resolved against the active theme.

To create a theme, add a `.json` file to `~/Library/Application Support/moth/themes/`. It inherits from `base` (Graphite if omitted), so only the colors that change are needed; the picker re-reads the folder every time it opens:

```json
{
  "name": "Graphite Orange",
  "base": "Graphite",
  "ui": { "accent": "#FF9F43" },
  "syntax": { "command": "#FF9F43" }
}
```

The available keys are in [`assets/themes/graphite.json`](assets/themes/graphite.json). An invalid color or a misspelled key invalidates the theme, and the error goes to the app log.

## Logging and security

App errors (an invalid theme or `hosts.json`, a tab that failed to open) are written to `~/Library/Logs/moth/moth.log`, in addition to standard output.

The app used to be called terminalapp. The first time Moth starts, if `~/Library/Application Support/terminalapp` exists and there's no `moth` folder yet, it's moved over wholesale, so settings, themes, hosts and the session are kept.

Programs can copy to the clipboard with OSC 52 (handy for copying from vim or tmux on a remote server), but can't read it: that way an SSH server or a file full of escape sequences can't grab what you copied.

## zsh integration

When a tab opens with zsh, the app installs `assets/shell/zsh/` into `~/Library/Application Support/moth/shell/zsh` and starts zsh with `ZDOTDIR` pointing there. That `.zshenv` restores your `ZDOTDIR`, sources your `.zshenv` and then the integration; `.zprofile` and `.zshrc` are still your own.

The integration emits OSC 133 (prompt and command start/end, with exit code; the end of prompt also carries the prompt's row count and whether the previous command failed), OSC 7 (current directory) and a custom OSC 7777: the line being edited on every keystroke, the command run, `HISTFILE`, `PATH` and the names of aliases, functions and builtins. The app picks these up from the PTY output before handing it to alacritty, and uses them to:

- highlight the line you're typing (command, subcommand, flags, strings, paths, operators, variables; unknown commands get a red squiggle);
- suggest in gray the rest of the most recent history command that starts the same way; → or End accepts it;
- move the cursor where you click on the line you're typing, like in an editor (including wrapped lines and wide characters);
- show the running command or the folder in the tab, and the path plus the last command's result and duration in the status bar;
- show the git status on every prompt (branch, `↓`/`↑` relative to upstream, `+` staged, `~` modified, `?` untracked, `!` conflicted). Picking a branch in the picker types and runs `git switch` in the shell, so the result or any error stays visible. Switching only works with the shell at the prompt, not while a command is running.

If you use macOS's default zsh prompt, the integration replaces it with one in the app's style: a dim header with the folder (the last three if the path is long), the branch, the previous command's result and duration, and the time; below it, `❯` in green, or red if the command failed, with a blank line between blocks. If you have your own prompt (starship, p10k…) it's left alone; to disable it anyway, set `MOTH_PROMPT=0` in your `.zshrc`.

If the app gets no locale (when launched from Finder), it passes the shell a UTF-8 `LANG` based on the macOS language, so accented and other non-ASCII characters work.

Other shells work the same, just without these features.

## Project structure

- `src/workspace.rs`: window, tab bar, panes, status bar with the branch picker, actions.
- `src/panes.rs`: a tab's pane tree (split, close, resize, neighbor in each direction).
- `src/session.rs`: saves and restores windows, tabs and panes.
- `src/notify.rs`: macOS notifications (via `osascript`).
- `src/terminal.rs`: PTY + `alacritty_terminal`, events to gpui.
- `src/terminal_view.rs`: paints the grid with gpui's text system, cursor, IME.
- `src/shell.rs`: zsh script installation, OSC detection in the PTY, history.
- `src/ssh.rs`: recognizes the `ssh` command, resolves it with `ssh -G` and reads `~/.ssh/config`.
- `src/hosts.rs`: `hosts.json` rules (label, color, production).
- `src/git.rs`: repo status (`git status --porcelain=v2`), branch list, switch command.
- `src/highlight.rs`: command-line lexer for highlighting.
- `src/picker.rs`: overlay picker (virtualized list, sections, keyboard) used by the palette, history and branches.
- `src/text_input.rs`: single-line text field with IME (the `ui` crate doesn't ship one).
- `src/fuzzy.rs`: fuzzy matching with scoring and positions for highlighting.
- `src/state.rs`: last folder, recent folders, theme, zoom and ⌥ as Meta.
- `src/log.rs`: errors to stderr and to `~/Library/Logs/moth/`.
- `src/glyphs.rs`: block elements (`▀▄█▐▛▜`…) and box-drawing lines (`─│╭╮`…) drawn as cell-filling rectangles, so logos like Claude Code's and TUI borders don't show gaps.
- `src/blocks.rs`: command blocks: invisible marks in the grid, each command's output and result.
- `src/completions.rs`: completion engine (parses the line, picks the source, ranks and applies).
- `src/mouse.rs`: mouse reporting to programs that request it (SGR and X10).
- `src/links.rs`: URLs and paths with line and column in the output, allowed schemes.
- `src/editor.rs`: opens files in the editor (detection, per-editor position arguments, terminal editors).
- `src/config.rs`: `config.json` (font, editor, scrollback), reloaded when the window regains focus.
- `src/keys.rs`: special keys → escape sequences.
- `src/tokens.rs`: design tokens, JSON theme loading with inheritance, spacing and radius metrics.
- `src/theme.rs`: applies the active theme to the app and to Zed's theme system (for the `ui` components), with live switching.
- `src/components.rs`: design components: `TabItem`, `Kbd`, `KindBadge`, `StatusDot`, `BlockStatus`.
- `src/palette.rs`: grid colors (ANSI, 256, truecolor) resolved through the tokens.

## Licenses

Zed's `ui` crate is GPL-3.0-or-later, so this project is too. The Geist fonts (`assets/fonts`) are OFL; the icons in `assets/icons` come from the Zed repo.
