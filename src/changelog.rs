//! ZenDesktop :: changelog.rs
//!
//! Embeds CHANGELOG.md at compile time so the app can show a
//! "What's New" dialog after an update without needing network access.

/// The full CHANGELOG.md content, embedded at compile time.
const RAW: &str = include_str!("../CHANGELOG.md");

/// Extracts the latest release section from the embedded CHANGELOG.
///
/// Returns `(version_string, body_text)` for the most recent `## [X.Y.Z]`
/// section, or `None` if no release section is found.
pub fn latest_release() -> Option<(&'static str, &'static str)> {
    let start_marker = "## [";
    let pos = RAW.find(start_marker)?;
    let after_marker = &RAW[pos + start_marker.len()..];

    // Parse version: everything up to "]"
    let ver_end = after_marker.find(']')?;
    let version = &after_marker[..ver_end];

    // Find the body: from after the header line to the next "## [" or EOF
    let body_start = after_marker[ver_end..].find('\n')? + ver_end + 1;
    let body_rest = &RAW[pos + body_start..];

    let next_section = body_rest.find("\n## [");
    let body = match next_section {
        Some(n) => &body_rest[..n],
        None => body_rest,
    };

    Some((version, body.trim()))
}

/// Truncates the release body to fit nicely in a MessageBox (~800 chars).
pub fn summary(body: &str, max_chars: usize) -> String {
    if body.len() <= max_chars {
        return body.to_string();
    }
    let mut truncated = body[..max_chars].to_string();
    // Cut at last newline to avoid chopping mid-word
    if let Some(pos) = truncated.rfind('\n') {
        truncated.truncate(pos);
    }
    truncated.push_str("\n\n... (see full changelog on GitHub)");
    truncated
}
