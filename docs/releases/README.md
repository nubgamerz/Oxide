# Release notes

One file per release, `<version>.md` (the version in the root `Cargo.toml`'s
`[workspace.package]`), written for musicians: what's new, what's fixed, and anything a user
must do after updating. A release isn't made without the file, and it is
published as the release's text on GitHub, under the release's own title (`Oxide
<version>`), so the file starts straight with its first paragraph (no heading of its own) and
reads on its own.

- A change that will ship adds its lines to the next version's notes in the same change.
- The version bump and its notes go in together.
- No one's own setup: the personal-names guard (`crates/manual/tests/personal_names.rs`) reads
  these files as it reads the manual and the README.

`docs/DEVELOPMENT.md`, "Making a release", has the rest.
