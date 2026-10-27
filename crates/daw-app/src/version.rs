//! The app's version and the commit it was built from (`build.rs`), for Help > About, the
//! status bar, `--version`, MCP's server info and `get_app_info`.

/// `major.minor.patch`: the workspace's version, or the build's `OXIDE_VERSION`.
pub const VERSION: &str = env!("OXIDE_VERSION");

/// The commit's short hash (`-modified` when built from a changed tree by the installer
/// script), or `unknown` without git.
pub const COMMIT: &str = env!("OXIDE_COMMIT");

/// "0.2.0 (c56cc31)".
pub fn text() -> String {
    format!("{VERSION} ({COMMIT})")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_version_is_the_workspaces_unless_a_build_overrides_it() {
        // A build for the installer check sets OXIDE_VERSION in its environment.
        if std::env::var_os("OXIDE_VERSION").is_none() {
            assert_eq!(super::VERSION, env!("CARGO_PKG_VERSION"));
        }
        assert_eq!(super::VERSION.split('.').count(), 3);
        assert!(!super::COMMIT.is_empty());
    }
}
