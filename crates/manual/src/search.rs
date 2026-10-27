//! The search index and its ranking live in the `manual-search` crate, which the app links
//! too (`search_manual`, dev plan 27.3) without the generator's Markdown and image crates;
//! this module keeps their old path (`manual::search`).

pub use manual_search::*;
