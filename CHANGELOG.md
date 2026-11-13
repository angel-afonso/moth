# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/angel-afonso/moth/compare/v0.5.0...v0.6.0) - 2026-11-13

### Added

- *(history)* filter the history search by folder and failures
- *(completions)* rank the history suggestion and add sources to the popup

### Other

- improve text selection

## [0.5.0](https://github.com/angel-afonso/moth/compare/v0.4.0...v0.5.0) - 2026-11-10

### Added

- *(agent)* add agent mode, which runs read-only commands on its own and asks for the rest
- *(links)* add a right-click menu to links and paths

## [0.4.0](https://github.com/angel-afonso/moth/compare/v0.3.0...v0.4.0) - 2026-11-08

### Added

- recover tabs when their volume comes back, and harden the shell integration
- *(agent)* label answers as AI and answer without reasoning

### Fixed

- *(links)* open file:// URLs with a drive letter on Windows
- *(agent)* keep requests alive until the agent answers
- *(update)* install the Linux binary, not the folder that holds it
- *(terminal)* keep the proportions of pictures drawn with blocks

## [0.3.0](https://github.com/angel-afonso/moth/compare/v0.2.0...v0.3.0) - 2026-11-03

### Added

- suggest fixes for failed commands and ask an ACP agent

### Documentation

- *(site)* one download button and the whole panes window
- add the project website

### Fixed

- *(agent)* keep the session until a cancelled turn ends

## [0.2.0](https://github.com/angel-afonso/moth/compare/v0.1.0...v0.2.0) - 2026-11-01

### Added

- replace app icon with terminal prompt design
- update automatically from GitHub releases
- run on Linux and Windows
