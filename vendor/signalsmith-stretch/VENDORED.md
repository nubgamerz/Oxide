# Vendored: signalsmith-stretch 0.1.3

The crates.io crate `signalsmith-stretch` 0.1.3 (Colin Marc, MIT), which wraps Signalsmith
Stretch 1.3.1 (Geraint Luff, MIT) and Signalsmith Linear (MIT). The workspace uses it through
`[patch.crates-io]` in the root `Cargo.toml`.

## Why

Upstream's default constructor seeds its random engine from `std::random_device`, and beyond a
2× stretch it randomises each bin's time factor. Two renders of the same clip at 2× or more
therefore differed from process to process. The DAW caches stretched clips and expects a
re-render (after reopening a project, say) to give the same audio, so the stretcher must be
deterministic.

## Changes from the crate

- `src/wrapper.cpp`: the instance is built with a fixed seed (`SignalsmithStretch(long seed)`).
- `Cargo.toml`: the crate's manifest without its examples and dev-dependencies.
- Left out: the crate's examples, upstream's `web/` demo and Signalsmith Linear's `tests/`.

Everything else is byte for byte the crate as published.
