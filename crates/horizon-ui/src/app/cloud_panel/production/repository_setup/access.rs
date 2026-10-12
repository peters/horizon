//! Account problems a proof can meet that the person fixes on GitHub, not in the
//! Dockerfile. Horizon names the cause from the proof's output and offers to let the
//! agent fix it in a browser panel the person can watch and take over; the next proof
//! shows whether it worked.
use horizon_core::cloud_runtime::github::publish::APP_PAGE;

/// Lines of a phase's output kept for a diagnosis, and the characters kept of each.
const TAIL_LINES: usize = 2000;
const TAIL_LINE_CHARS: usize = 4096;

/// The newest lines of a proof phase's output, kept in memory so a diagnosis never
/// reads the log file back from the run folder, where the agent can write.
#[derive(Default)]
pub(super) struct Tail(std::collections::VecDeque<String>);

impl Tail {
    pub(super) fn push(&mut self, line: &str) {
        if self.0.len() == TAIL_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line.chars().take(TAIL_LINE_CHARS).collect());
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// An account problem behind a failed build: GitHub's own words in the output, or,
    /// when ghcr.io only refused an image the build pulls, GitHub's packages API, asked
    /// with Horizon's login in `docker_config` until `cancelled` says stop.
    pub(super) fn diagnose(&self, docker_config: &std::path::Path, cancelled: &dyn Fn() -> bool) -> Option<Fix> {
        Fix::find(self.0.iter()).or_else(|| {
            let refused = self.refused_ghcr_images()?;
            horizon_core::cloud_runtime::github::publish::restricting_organization(docker_config, &refused, cancelled)
                .map(|organization| Fix::GrantOrganization { organization })
        })
    }

    /// The build's own lines that name a `ghcr.io` image ghcr.io refused, or `None` when the
    /// build failed otherwise, such as in a `RUN` step after the base image was pulled.
    /// An error that names its image counts only for a `ghcr.io` one; buildx's older
    /// refusal names none, and then the `ghcr.io` image of the step that failed is the
    /// candidate.
    fn refused_ghcr_images(&self) -> Option<String> {
        let lines: Vec<&str> = self.0.iter().map(String::as_str).collect();
        // The images of the build steps that failed (`#N ERROR`), as their metadata lines name them.
        let failed_steps: Vec<&str> = lines.iter().filter_map(|line| failed_step(line)).collect();
        let resolved = || {
            lines.iter().copied().filter(|line| {
                metadata_line(line)
                    && line.contains("ghcr.io/")
                    && line
                        .split_once(' ')
                        .is_some_and(|(step, _)| failed_steps.contains(&step))
            })
        };
        let mut refused: Vec<&str> = Vec::new();
        for (index, line) in lines.iter().copied().enumerate() {
            let next = lines.get(index + 1).copied().unwrap_or_default();
            if !error_line(line) || !(refusal(line) || next.starts_with("denied:") || next.starts_with("unauthorized:"))
            {
                continue;
            }
            match failing_image(line) {
                Some(image) if image.to_ascii_lowercase().starts_with("ghcr.io/") => refused.push(line),
                Some(_) => {}
                None if line.contains("ghcr.io/") => refused.push(line),
                None => refused.extend(resolved()),
            }
        }
        (!refused.is_empty()).then(|| refused.join("\n"))
    }
}

/// buildx's own line for an image the build resolves: `#N [internal] load metadata for IMAGE`.
fn metadata_line(line: &str) -> bool {
    line.split_once(' ').is_some_and(|(step, rest)| {
        step.len() > 1
            && step.starts_with('#')
            && step[1..].bytes().all(|byte| byte.is_ascii_digit())
            && rest.starts_with("[internal] load metadata for ")
    })
}

/// The image a build's error line says it failed to resolve, as in
/// `ERROR: failed to build: failed to solve: IMAGE: failed to resolve ...`.
fn failing_image(error: &str) -> Option<&str> {
    let (_, rest) = error.split_once("failed to solve: ")?;
    let candidate = rest.split(": ").next()?;
    candidate.contains('/').then_some(candidate)
}

/// The step of a buildx line that reports its failure, `#N ERROR: ...`.
fn failed_step(line: &str) -> Option<&str> {
    let (step, rest) = line.split_once(' ')?;
    let numbered = step.len() > 1 && step.starts_with('#') && step[1..].bytes().all(|byte| byte.is_ascii_digit());
    (numbered && rest.starts_with("ERROR")).then_some(step)
}

/// The build's own failure line, not a failed `RUN` step's.
fn error_line(line: &str) -> bool {
    line.starts_with("ERROR:") && !line.contains("process \"")
}

/// Whether a registry error line says the registry refused access.
fn refusal(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    ["denied", "403 forbidden", "401 unauthorized", "unauthorized:"]
        .iter()
        .any(|word| line.contains(word))
}

/// A GitHub organization name: letters, digits and single inner hyphens, at most 39.
fn organization_name(name: &str) -> bool {
    (1..=39).contains(&name.len())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// What GitHub's package registry answers when an organization limits OAuth apps.
const RESTRICTED: &str = "organization has enabled OAuth App access restrictions";

/// A cause Horizon recognised in a failed proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Fix {
    /// The organization limits OAuth apps, so Horizon's GitHub login cannot pull its
    /// private images until an owner grants Horizon's app.
    GrantOrganization { organization: String },
}

impl Fix {
    /// The first recognised cause in a proof's output: ghcr.io's refusal of an image the
    /// organization owns. The refusal is in the build's own `ERROR:` line or on the line
    /// after it, and the image must be one the build pulled from `ghcr.io/ORGANIZATION/`;
    /// a failed `RUN` step that prints the words, or another registry, is no such cause.
    pub(super) fn find<'a>(lines: impl IntoIterator<Item = &'a String>) -> Option<Self> {
        let lines: Vec<&str> = lines.into_iter().map(String::as_str).collect();
        // The image the error names is the one refused; only an error that names none
        // (buildx's older one-line form) falls back to the image of the step that failed.
        let failed_steps: Vec<&str> = lines.iter().filter_map(|line| failed_step(line)).collect();
        let pulled = |organization: &str, error: &str| {
            let image = format!("ghcr.io/{}/", organization.to_ascii_lowercase());
            match failing_image(error) {
                Some(failing) => failing.to_ascii_lowercase().starts_with(&image),
                None => lines.iter().any(|line| {
                    metadata_line(line)
                        && line.to_ascii_lowercase().contains(&image)
                        && line
                            .split_once(' ')
                            .is_some_and(|(step, _)| failed_steps.contains(&step))
                }),
            }
        };
        lines.iter().enumerate().find_map(|(index, error)| {
            if !error_line(error) {
                return None;
            }
            let refusal = [Some(*error), lines.get(index + 1).copied()]
                .into_iter()
                .flatten()
                .find(|line| line.contains(RESTRICTED))?;
            let (before, _) = refusal.split_once(RESTRICTED)?;
            // "... the `ORGANIZATION` organization has enabled ..."
            let organization = before.trim_end().strip_suffix('`')?.rsplit_once('`')?.1;
            (organization_name(organization) && pulled(organization, error)).then(|| Self::GrantOrganization {
                organization: organization.to_owned(),
            })
        })
    }

    pub(super) fn title(&self) -> String {
        match self {
            Self::GrantOrganization { organization } => format!("{organization} blocks Horizon's GitHub app"),
        }
    }

    pub(super) fn explanation(&self) -> String {
        match self {
            Self::GrantOrganization { organization } => format!(
                "The organization limits which apps may read its private images, so the build cannot pull its base image. The Dockerfile is fine: an owner of {organization} grants Horizon once on GitHub."
            ),
        }
    }

    /// The page where the fix is made.
    pub(super) fn url(&self) -> &'static str {
        match self {
            Self::GrantOrganization { .. } => APP_PAGE,
        }
    }

    /// What the agent is told with the failed proof, so it changes no file for it.
    pub(super) fn report(&self) -> String {
        match self {
            Self::GrantOrganization { organization } => format!(
                "Horizon proof: Build image failed because the {organization} organization limits OAuth apps, so Horizon's GitHub login cannot pull its private image. The Dockerfile is not the cause: change no file for this. Horizon offers the person a fix; wait for Horizon's next message."
            ),
        }
    }

    /// What the agent is asked once the person lets it make the fix in `panel`.
    pub(super) fn task(&self, panel: &str) -> String {
        match self {
            Self::GrantOrganization { organization } => format!(
                "The person asked you to fix this for them: grant Horizon's GitHub OAuth app access to the {organization} organization. Use the Horizon browser tools on the existing browser panel {panel} (named GitHub access), already open at {url}; do not create another panel. Take a snapshot first. If GitHub shows a sign-in, password, two-factor or passkey page, set question in the status file to \"Sign in to GitHub in the browser panel, then press Done\", call browser_handoff with a short reason and wait; never type a password or code yourself, and set question back to null after. On the page, under Organization access, click Grant next to {organization} and confirm it; click nothing else. If it offers only Request, the person is not an owner: click Request, then set question to \"An owner of {organization} must approve Horizon on GitHub\" and stop. If {organization} shows a red cross and no button, an owner denied Horizon before: navigate the same panel to {policy}, open Horizon in the list and click Grant access. When {organization} shows as granted, increase proof_request by one so Horizon proves again.",
                url = self.url(),
                policy = policy_page(organization)
            ),
        }
    }
}

/// Where an owner changes an organization's decision on an OAuth app, including one
/// it denied before; the person's own app page then shows no Grant button.
fn policy_page(organization: &str) -> String {
    format!("https://github.com/organizations/{organization}/settings/oauth_application_policy")
}

/// Who makes the fix in the browser panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Fixer {
    Agent,
    Person,
}

/// What the agent is told when the person fixed it themselves.
pub(super) const FIXED: &str =
    "The person made the fix on GitHub. Increase proof_request by one so Horizon proves again; change nothing else.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restricted_organization_names_the_fix() {
        let log = [
            "#4 [internal] load metadata for ghcr.io/fintermobilityas/runtime:1.0".to_owned(),
            "#4 ERROR: error from registry: permission_denied: denied".to_owned(),
            "ERROR: failed to solve: error from registry: permission_denied: Although you appear to have the correct authorization credentials, the `fintermobilityas` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited.".to_owned(),
        ];
        let fix = Fix::find(&log).unwrap();
        assert_eq!(
            fix,
            Fix::GrantOrganization {
                organization: "fintermobilityas".into()
            }
        );
        assert_eq!(fix.title(), "fintermobilityas blocks Horizon's GitHub app");
        assert!(
            fix.url()
                .starts_with("https://github.com/settings/connections/applications/")
        );
        assert!(fix.task("panel-7").contains("browser panel panel-7"));
        assert!(fix.task("panel-7").contains("never type a password"));
        assert!(
            fix.task("panel-7")
                .contains("https://github.com/organizations/fintermobilityas/settings/oauth_application_policy"),
            "an organization that denied Horizon before is granted on its policy page"
        );
    }

    #[test]
    fn an_image_less_refusal_of_another_registrys_step_names_no_fix() {
        let log = [
            "#2 [internal] load metadata for ghcr.io/acme/base:1".to_owned(),
            "#2 DONE 0.4s".to_owned(),
            "#3 [internal] load metadata for registry.example/tools:1".to_owned(),
            "#3 ERROR: error from registry: denied".to_owned(),
            "ERROR: failed to solve: error from registry: denied: the `acme` organization has enabled OAuth App access restrictions".to_owned(),
        ];
        assert_eq!(Fix::find(&log), None);
    }

    #[test]
    fn newer_buildx_puts_the_refusal_on_the_line_after_the_error() {
        let log = [
            "ERROR: failed to build: failed to solve: ghcr.io/acme/base:1: failed to resolve source metadata for ghcr.io/acme/base:1: unexpected status from HEAD request to https://ghcr.io/v2/acme/base/manifests/1: 403 Forbidden".to_owned(),
            "denied: Although you appear to have the correct authorization credentials, the `acme` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited.".to_owned(),
        ];
        assert_eq!(
            Fix::find(&log),
            Some(Fix::GrantOrganization {
                organization: "acme".into()
            })
        );
    }

    fn tail(lines: &[&str]) -> Tail {
        let mut tail = Tail::default();
        for line in lines {
            tail.push(line);
        }
        tail
    }

    #[test]
    fn github_is_asked_only_about_an_image_ghcr_refused() {
        let refused = tail(&[
            "#2 [internal] load metadata for ghcr.io/acme/base:1",
            "ERROR: failed to build: failed to solve: ghcr.io/acme/base:1: failed to resolve source metadata for ghcr.io/acme/base:1: unexpected status from HEAD request to https://ghcr.io/v2/acme/base/manifests/1: 403 Forbidden",
            "denied: denied",
        ]);
        assert!(refused.refused_ghcr_images().unwrap().contains("ghcr.io/acme/base:1"));
        // The base image was pulled; a later RUN step failed and printed an image name.
        let pulled = tail(&[
            "#2 [internal] load metadata for ghcr.io/acme/base:1",
            "#2 DONE 0.4s",
            "#7 0.31 ghcr.io/bigorg/x denied",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c echo ghcr.io/bigorg/x denied && false\" did not complete successfully: exit code: 1",
        ]);
        assert_eq!(pulled.refused_ghcr_images(), None);
        // buildx's older refusal names no image: the images the build resolved are asked about.
        let legacy = tail(&[
            "#2 [internal] load metadata for ghcr.io/acme/base:1",
            "#2 ERROR: error from registry: permission_denied: denied",
            "ERROR: failed to solve: error from registry: permission_denied: denied",
        ]);
        assert!(legacy.refused_ghcr_images().unwrap().contains("ghcr.io/acme/base:1"));
        // The same image-less refusal, but of another registry's step after a ghcr.io one resolved.
        let other_step = tail(&[
            "#2 [internal] load metadata for ghcr.io/acme/base:1",
            "#2 DONE 0.4s",
            "#3 [internal] load metadata for registry.example/tools:1",
            "#3 ERROR: error from registry: permission_denied: denied",
            "ERROR: failed to solve: error from registry: permission_denied: denied",
        ]);
        assert_eq!(other_step.refused_ghcr_images(), None);
        // An error that names another registry's image is never GitHub's.
        let elsewhere = tail(&[
            "#2 [internal] load metadata for ghcr.io/acme/base:1",
            "ERROR: failed to solve: registry.example/acme/base:1: failed to resolve source metadata: 403 Forbidden",
        ]);
        assert_eq!(elsewhere.refused_ghcr_images(), None);
        let home = tempfile::tempdir().unwrap();
        assert_eq!(pulled.diagnose(home.path(), &|| false), None);
    }

    #[test]
    fn other_failures_name_no_fix() {
        let log = [
            "ERROR: failed to solve: process \"/bin/sh -c dotnet restore\" did not complete successfully".to_owned(),
            "the `` organization has enabled OAuth App access restrictions".to_owned(),
            "the `a b` organization has enabled OAuth App access restrictions".to_owned(),
            // A RUN step that only prints the words is no refusal from the registry.
            "#9 0.42 the `elsewhere` organization has enabled OAuth App access restrictions".to_owned(),
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c echo error from registry: the `elsewhere` organization has enabled OAuth App access restrictions\" did not complete successfully: exit code: 1".to_owned(),
            "ERROR: failed to solve: error from registry: permission_denied: the `-bad-` organization has enabled OAuth App access restrictions".to_owned(),
            // Another registry may say anything, even after a ghcr.io image of the same name
            // resolved: only ghcr.io's refusal of the organization's image counts.
            "#2 [internal] load metadata for ghcr.io/acme/base:1".to_owned(),
            "ERROR: failed to solve: registry.example/acme/base:1: failed to resolve source metadata for registry.example/acme/base:1: 403 Forbidden".to_owned(),
            "denied: the `acme` organization has enabled OAuth App access restrictions".to_owned(),
        ];
        assert_eq!(Fix::find(&log), None);
    }
}
