use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::project::Project;

const MARKER_FILE: &str = "session-start.json";

/// How far ahead of the server's own start a delivery may sit and still belong to the same
/// session. The hook and the server are launched together in an order neither of them controls,
/// so the marker can land on either side of the server coming up.
const TOLERANCE_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    delivered_at_ms: u64,
}

pub fn marker_path(project: &Project) -> PathBuf {
    crate::lsp::cache::cache_dir(project).join(MARKER_FILE)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Records that the SessionStart hook has put the manual into the agent's context.
pub fn record_delivery(project: &Project) -> std::io::Result<()> {
    let path = marker_path(project);
    if let Some(parent) = path.parent() {
        crate::lsp::cache::prepare_dir(parent)?;
    }
    let marker = Marker {
        delivered_at_ms: now_ms(),
    };
    let mut text = serde_json::to_string(&marker).map_err(std::io::Error::other)?;
    text.push('\n');
    std::fs::write(path, text)
}

/// Whether a delivery recorded by the hook is recent enough to belong to the session the server
/// starting at `server_started_ms` is serving. A marker left behind by an earlier session, and a
/// project whose client never runs the hook at all, both answer false.
pub fn delivered_for_session(project: &Project, server_started_ms: u64) -> bool {
    let Ok(raw) = std::fs::read_to_string(marker_path(project)) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<Marker>(&raw) else {
        return false;
    };
    marker.delivered_at_ms.saturating_add(TOLERANCE_MS) >= server_started_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        (dir, project)
    }

    fn write_marker(project: &Project, delivered_at_ms: u64) {
        let path = marker_path(project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_string(&Marker { delivered_at_ms }).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn no_marker_means_no_delivery() {
        let (_dir, project) = project();
        assert!(!delivered_for_session(&project, now_ms()));
    }

    #[test]
    fn a_recorded_delivery_counts_for_a_server_that_started_alongside_it() {
        let (_dir, project) = project();
        record_delivery(&project).unwrap();
        assert!(delivered_for_session(&project, now_ms()));
    }

    #[test]
    fn a_delivery_recorded_after_the_server_started_still_counts() {
        let (_dir, project) = project();
        let started = now_ms();
        write_marker(&project, started + 5_000);
        assert!(delivered_for_session(&project, started));
    }

    #[test]
    fn a_marker_from_an_earlier_session_is_ignored() {
        let (_dir, project) = project();
        let started = now_ms();
        write_marker(&project, started.saturating_sub(TOLERANCE_MS + 1));
        assert!(!delivered_for_session(&project, started));
    }

    #[test]
    fn an_unreadable_marker_is_ignored() {
        let (_dir, project) = project();
        let path = marker_path(&project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{ not json").unwrap();
        assert!(!delivered_for_session(&project, now_ms()));
    }

    #[test]
    fn recording_creates_a_gitignored_cache_directory() {
        let (dir, project) = project();
        project.bootstrap().unwrap();
        record_delivery(&project).unwrap();

        assert!(marker_path(&project).is_file());
        let gitignore =
            std::fs::read_to_string(dir.path().join(".biskit").join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|line| line.trim() == "cache/"));
    }
}
