//! The state report that Horizon gives a pi panel at its launch.
//!
//! pi loads an extension only from a file that exists: a path that names no file
//! stops pi before its first frame (`pi -e /missing` is fatal). Horizon therefore
//! writes the report into its own home, beside the plugin folder of the work
//! hooks, and a launch appends it to the command line only while it is a file.
//!
//! The extension is Horizon's own file: it holds no path, no token and no name
//! of this computer, so the same content serves every home. It reports the
//! events of the process it runs in to the work entry of the Horizon that
//! started that process, with the identity that the launch put in the
//! environment of that one process.

use std::io;
use std::path::Path;

use super::skill_files::PI_REPORT_EXTENSION;

/// Writes the report of a pi panel, and only when the file on disk differs.
pub(super) fn install(path: &Path) -> io::Result<bool> {
    super::sync_file_if_changed(path, PI_REPORT_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_is_written_once_and_holds_no_path_of_this_computer() {
        let directory = tempfile::tempdir().expect("report");
        let path = directory.path().join("work-report/pi-report.mjs");
        assert!(install(&path).expect("install"), "the first install writes the file");
        assert!(
            !install(&path).expect("install"),
            "the same content is not written again"
        );
        let text = std::fs::read_to_string(&path).expect("report file");
        assert_eq!(text, PI_REPORT_EXTENSION);
        assert!(
            !text.contains(directory.path().to_str().expect("path")),
            "the report names no directory of this computer"
        );
        assert!(
            text.contains("HORIZON_WORK_EXECUTABLE") && !text.contains("HORIZON_TITLE"),
            "the report starts the work entry of its launch"
        );
        // A pi panel must never fail to load the file, so the module must be
        // valid on its own: a default export and no side effect at import.
        assert!(text.contains("export default function activate"));
    }
}
