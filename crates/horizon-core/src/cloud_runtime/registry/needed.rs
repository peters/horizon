//! The image repository a cloud's profile names, as Cloud settings shows it before it
//! is bound: who can publish the cloud's image there, and that workers have no pull
//! credential for it.
use super::Settings;
use crate::cloud_runtime::{github::publish, repository::launch::quick_start};

/// An image repository that a cloud needs and Container registry does not bind yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needed {
    /// The registry host and repository, without a tag or digest.
    pub repository: String,
    pub publishing: Publishing,
}

/// Who can publish a cloud's image to a repository without a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Publishing {
    /// `ghcr.io`: Horizon publishes as this GitHub account.
    GitHub(String),
    /// `ghcr.io`: the cloud card asks once, at the first push.
    AskOnCard,
    /// Another registry: a publishing credential bound in Container registry.
    Credential,
}

/// The repository of `image` when Container registry has no binding for it, or `None`
/// when there is nothing to bind: the public base image, an image without an explicit
/// registry host, or a repository that is already bound.
#[must_use]
pub fn needed(settings: &Settings, image: &str) -> Option<Needed> {
    if quick_start::is_public_base(image) {
        return None;
    }
    let repository = super::repository(image).ok()?;
    let bound = settings
        .registries
        .as_ref()
        .is_some_and(|config| config.bindings.iter().any(|binding| binding.repository == repository));
    if bound {
        return None;
    }
    let publishing = if publish::publishes_to_ghcr(image) {
        publish::publisher(&settings.docker_config).map_or(Publishing::AskOnCard, Publishing::GitHub)
    } else {
        Publishing::Credential
    };
    Some(Needed {
        repository: repository.to_owned(),
        publishing,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::*;

    #[test]
    fn only_an_unbound_repository_with_a_registry_host_is_needed() {
        let (_root, settings) = fixture();
        assert_eq!(
            needed(&settings, "registry.example/team/app:9f3c2a1"),
            Some(Needed {
                repository: "registry.example/team/app".into(),
                publishing: Publishing::Credential,
            })
        );
        assert_eq!(
            needed(&settings, "ghcr.io/acme/worker:main").map(|needed| needed.publishing),
            Some(Publishing::AskOnCard),
            "without a stored chain the card asks at the first push"
        );
        for image in [
            "registry.example/team/worker:latest",
            quick_start::IMAGE,
            "acme/worker:1",
            "not an image",
        ] {
            assert_eq!(needed(&settings, image), None, "{image}");
        }
    }
}
