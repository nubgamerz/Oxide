# Contributing to Oxide

Thanks for wanting to help. Oxide is a small, lightweight DAW for bedroom and home musicians,
looked after by one maintainer ([@nubgamerz](https://github.com/nubgamerz)), so a short, clear
report or a focused pull request goes a long way.

## Reporting a bug

Open an issue with the **Bug report** template. The more of this you can include, the quicker
it can be fixed:

- the Oxide version (Help > About),
- your Windows version,
- the audio driver (WASAPI or ASIO) and the buffer size,
- the plug-ins involved, if any,
- the steps that make it happen, and what you expected to happen instead.

Attach a project only if you're happy to share it. Issues are public, so never attach anything
you wouldn't want everyone to see.

For "how do I...?" questions, the manual is the first stop (Help > Manual in the app, or F1
over the part you're looking at; its [sources](docs/manual/) are here too). If it doesn't
answer it, ask in [Discussions](https://github.com/nubgamerz/Oxide/discussions) rather than
opening an issue.

## Suggesting a feature

Open an issue with the **Feature request** template. Tell us what you're trying to do as a
musician, and how you get it done today, not only the button you'd like. There are often other
ways to solve the same problem.

Oxide stays simple on purpose, so not every good idea fits. A "no" is about the shape of the
app, not about you or your idea.

## Pull requests

- **Small fixes** (a typo, a wrong label, an obvious bug): just open the pull request.
- **Anything bigger**: open an issue first so we can agree on the approach before you spend
  your time.

How they are reviewed:

- The maintainer reviews every pull request, with an AI assistant's help, against the
  project's plan and its design rules (the two below are the heart of them).
- Only the maintainer merges. Nobody can push to `master` directly; it changes only through
  reviewed pull requests.
- A review can ask for changes, or decline a change that doesn't fit. That is normal.

What a pull request must pass:

1. `cargo fmt` (formatting is enforced).
2. The quick tier, run locally on Windows:
   `pwsh -NoProfile -File scripts/check.ps1 -Tier quick`. It builds, checks the manual's
   references, runs every test and runs clippy. Say in the pull request that it passed.

There are no automatic checks on pull requests: the maintainer reads each one and runs the
quick tier on it while reviewing.

## The two rules

Most of the code is free to change. Two things are not:

- **What you hear is what you export.** Playback and export go through one processing path,
  with the same engine, plug-in instances and block size. Don't add a code path that only
  exports, or only plays.
- **The audio thread never waits, allocates or frees.** No locks it can block on, and no memory
  allocated or released while audio is being processed. One hiccup there is a click in
  somebody's recording.

[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) describes the workspace, how it's built and the
checks; if a change touches the engine, the real-time code or exports, say so in the pull
request and expect questions.

## New features need two more things

If users can do something new, the change also includes:

- **An MCP action** for it, so an AI assistant can do whatever a person can. It is one action
  that the app's own UI calls too: register it in `crates/daw-core/src/api/actions/<area>.rs`,
  add a test in `actions/tests.rs`, then regenerate the reference with
  `$env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current` (in PowerShell).
- **Its manual page**, updated in the same change.
  [docs/manual/README.md](docs/manual/README.md) explains how the pages are written.

If the change ships, add its lines to the next version's notes in `docs/releases/`.

## Building from source

[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) covers building, running, the checks and what's
already there. Oxide targets 64-bit Windows and VST3 plug-ins.

## Licence

Oxide is free software under the [GNU General Public License v3](LICENSE) (with one added term,
in [NOTICE](NOTICE): the licence doesn't cover the name "Oxide" or its logo). Anyone may use,
change and share it under the GPL, and the music made with it is theirs.

By opening a pull request you agree that:

- your contribution is licensed under the GNU General Public License v3, as Oxide is;
- you also grant the maintainer (nubgamerz) a perpetual, worldwide, non-exclusive, royalty-free,
  irrevocable licence to use, copy, change, distribute, sublicense and relicense your
  contribution under any terms, including commercial ones, so that Oxide's licence can change
  in the future without asking every contributor again;
- the contribution is your own work, or you have the right to submit it under these terms.
