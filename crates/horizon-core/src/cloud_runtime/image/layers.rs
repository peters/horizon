//! Local image builds, including same-worker sibling recipes layered on the image before them.
use super::{Error, Event, Progress, Result, Runner, TIMEOUT, contained, contract::finish_contract, valid_tag};
use crate::cloud_runtime::siblings::SiblingError;
use horizon_cloud::{Build, Cancellation, CloudError};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// The build argument a sibling Dockerfile declares as `ARG HORIZON_BASE` and builds `FROM`.
const BASE_ARGUMENT: &str = "HORIZON_BASE";
/// Local repository for intermediate layers. They are never pushed, and their `-<index>`
/// suffix keeps their tags apart from a pushed image's tag.
const LAYER_REPOSITORY: &str = "horizon-layer";
const CLEANUP_TIMEOUT: Duration = Duration::from_mins(1);
/// Label key prefix stamped on each intermediate image. An image built on one inherits its
/// labels, which proves the ancestry even when the base only changed metadata.
const STAMP: &str = "horizon.sibling-base";

/// A sibling recipe from its committed snapshot, built on the image before it.
pub struct Layer<'a> {
    pub alias: &'a str,
    pub build: &'a Build,
    pub source: &'a Path,
}

/// A recipe's Dockerfile and context, both inside its committed snapshot.
pub(super) struct Recipe {
    dockerfile: PathBuf,
    context: PathBuf,
    platform: String,
}

impl Recipe {
    pub(super) fn new(source: &Path, build: &Build) -> Result<Self> {
        Ok(Self {
            context: contained(source, &build.context)?,
            dockerfile: contained(source, &build.dockerfile)?,
            platform: build.platform.clone(),
        })
    }
}

/// Local builds through `docker`, which makes each Docker command with its client options.
pub(super) struct Builder<'a, D> {
    pub docker: D,
    pub runner: &'a Runner<'a>,
}

impl<D: Fn() -> Command> Builder<'_, D> {
    pub(super) fn build(&self, image: &str, recipe: &Recipe, arguments: &[String]) -> Result<()> {
        self.build_layer(image, recipe, arguments, &[])
    }

    /// Builds with `layering`, the base and stamp arguments of a layered build, after the
    /// Horizon build arguments.
    fn build_layer(&self, image: &str, recipe: &Recipe, arguments: &[String], layering: &[String]) -> Result<()> {
        self.runner
            .run(
                "image build",
                &mut self.build_command(image, recipe, arguments, layering),
                TIMEOUT,
            )
            .map(|_| ())
    }

    fn build_command(&self, image: &str, recipe: &Recipe, arguments: &[String], layering: &[String]) -> Command {
        let mut command = (self.docker)();
        command
            .args([
                "buildx",
                "build",
                "--load",
                "--provenance=false",
                "--progress=plain",
                "--platform",
                &recipe.platform,
            ])
            .args(arguments)
            .args(layering)
            .args(["--tag", image, "--file"])
            .arg(&recipe.dockerfile)
            .arg(&recipe.context);
        command
    }

    pub(super) fn image_id(&self, image: &str) -> Result<String> {
        let image_id = self.runner.run(
            "local image identity",
            (self.docker)().args(["image", "inspect", "--format", "{{.Id}}", image]),
            Duration::from_secs(30),
        )?;
        Ok(image_id.trim().to_owned())
    }

    /// Builds the primary recipe and then each layer on the image before it, and returns the
    /// final image's identity. The last layer gets `image`; the rest get local tags derived
    /// from `tag`, which are removed before building, so an interrupted attempt leaves no
    /// stale base, and afterwards whether or not the build succeeded.
    pub(super) fn build_layers(
        &self,
        image: &str,
        tag: &str,
        primary: &Recipe,
        layers: &[(&str, Recipe)],
        arguments: &[String],
    ) -> Result<String> {
        self.require_local_bases()?;
        let local = (0..layers.len())
            .map(|index| layer_image(tag, index))
            .collect::<Result<Vec<_>>>()?;
        if !self.remove_layers(&local, self.runner)? {
            return Err(Error::Invalid(
                "A previous attempt's intermediate sibling images could not be removed; remove the images named in the output with `docker image rm` and retry",
            ));
        }
        let nonce = crate::cloud_runtime::new_id();
        let stamp = |index: usize| (format!("{STAMP}.{index}"), nonce.clone());
        let stamped = |index: usize| {
            let (key, value) = stamp(index);
            ["--label".to_owned(), format!("{key}={value}")]
        };
        let built = (|| {
            let Some(first) = local.first() else {
                self.build(image, primary, arguments)?;
                return self.image_id(image);
            };
            self.build_layer(first, primary, arguments, &stamped(0))?;
            for (index, (alias, recipe)) in layers.iter().enumerate() {
                (self.runner.emit)(Event::Progress(Progress::activity(format!(
                    "Building sibling {alias} on the image before it"
                ))));
                let mut layering = vec!["--build-arg".to_owned(), format!("{BASE_ARGUMENT}={}", local[index])];
                let target = match local.get(index + 1) {
                    Some(next) => {
                        layering.extend(stamped(index + 1));
                        next.as_str()
                    }
                    None => image,
                };
                self.build_layer(target, recipe, arguments, &layering)?;
                let (key, value) = stamp(index);
                if self.labels(target)?.get(&key) != Some(&value) {
                    return Err(SiblingError::Base((*alias).to_owned()).into());
                }
            }
            self.image_id(image)
        })();
        // Removal runs even after cancellation, which ends the build, not its cleanup.
        let cancel = Cancellation::default();
        let cleanup = Runner {
            cancel: &cancel,
            emit: self.runner.emit,
            secrets: Vec::new(),
        };
        let cleanup = self.remove_layers(&local, &cleanup).and_then(|removed| {
            if removed {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Intermediate sibling image cleanup failed; remove the images named in the output with `docker image rm`",
                ))
            }
        });
        if let (Err(Error::Provider(CloudError::Cancelled)), Err(failed)) = (&built, &cleanup) {
            // Cancellation stays the outcome callers recognize; the next attempt removes the
            // leftover tags before it builds.
            (self.runner.emit)(Event::Output(format!("{failed}")));
            return built;
        }
        finish_contract(built, cleanup)
    }

    /// The labels of a local image, including those it inherited from its base.
    fn labels(&self, image: &str) -> Result<BTreeMap<String, String>> {
        let labels = self.runner.run(
            "local image labels",
            (self.docker)().args(["image", "inspect", "--format", "{{json .Config.Labels}}", image]),
            Duration::from_secs(30),
        )?;
        serde_json::from_str::<Option<BTreeMap<String, String>>>(labels.trim())
            .map(Option::unwrap_or_default)
            .map_err(|_| Error::Invalid("Invalid local image labels"))
    }

    /// Only the `docker` driver builds on images in the local store; another driver would
    /// look for the intermediate layer in a registry instead.
    fn require_local_bases(&self) -> Result<()> {
        let builder = self.runner.run(
            "image builder",
            (self.docker)().args(["buildx", "inspect"]),
            Duration::from_secs(30),
        )?;
        if builder_driver(&builder) == Some("docker") {
            Ok(())
        } else {
            Err(Error::Invalid(
                "Same-worker siblings build on a local image, which needs the docker buildx driver; run `docker buildx use default` and retry",
            ))
        }
    }

    /// Removes the `local` tags with one command and verifies their absence with another, so
    /// a hung daemon costs two timeouts whatever the number of siblings. Returns whether all
    /// are gone; the ones that remain are named in the output.
    fn remove_layers(&self, local: &[String], runner: &Runner<'_>) -> Result<bool> {
        // One missing tag fails the command after the others are removed; the listing decides.
        let _ = runner.run(
            "intermediate image cleanup",
            (self.docker)().args(["image", "rm"]).args(local),
            CLEANUP_TIMEOUT,
        );
        let mut listing = (self.docker)();
        listing.args(["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"]);
        for image in local {
            listing.arg("--filter").arg(format!("reference={image}"));
        }
        let listed = runner.run("intermediate image cleanup verification", &mut listing, CLEANUP_TIMEOUT)?;
        let remaining: Vec<_> = listed.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        if remaining.is_empty() {
            return Ok(true);
        }
        (runner.emit)(Event::Output(format!(
            "Intermediate sibling images remain: {}",
            remaining.join(" ")
        )));
        Ok(false)
    }
}

fn layer_image(tag: &str, index: usize) -> Result<String> {
    let layer = format!("{tag}-{index}");
    if !valid_tag(&layer) {
        return Err(Error::Invalid("Invalid intermediate image tag"));
    }
    Ok(format!("{LAYER_REPOSITORY}:{layer}"))
}

fn builder_driver(inspection: &str) -> Option<&str> {
    inspection
        .lines()
        .find_map(|line| line.strip_prefix("Driver:"))
        .map(str::trim)
}

#[cfg(all(test, unix))]
mod tests;
