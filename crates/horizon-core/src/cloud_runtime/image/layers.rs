//! Local image builds, including same-worker sibling recipes layered on the image before them.
use super::{Error, Event, Progress, Result, Runner, TIMEOUT, contained, contract::finish_cleanup, valid_tag};
use crate::cloud_runtime::siblings::SiblingError;
use horizon_cloud::{Build, Cancellation};
use std::{
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
/// One step of `docker history` per line, escaped so a multi-line command stays one line.
/// Metadata-only steps appear too, and a cached rebuild reproduces every line.
const HISTORY_FORMAT: &str = "{{json .CreatedAt}} {{json .CreatedBy}} {{json .Comment}}";

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

    /// Builds with `layering`, the base argument of a layered build, after the
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
        let built = (|| {
            self.build(local.first().map_or(image, String::as_str), primary, arguments)?;
            for (index, (alias, recipe)) in layers.iter().enumerate() {
                (self.runner.emit)(Event::Progress(Progress::activity(format!(
                    "Building sibling {alias} on the image before it"
                ))));
                let target = local.get(index + 1).map_or(image, String::as_str);
                let layering = ["--build-arg".to_owned(), format!("{BASE_ARGUMENT}={}", local[index])];
                let base = self.ancestry(&local[index])?;
                self.build_layer(target, recipe, arguments, &layering)?;
                if !self.ancestry(target)?.descends_from(&base) {
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
            secrets: self.runner.secrets.clone(),
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
        // After a cancellation, the next attempt removes the leftover tags before it builds.
        finish_cleanup(built, cleanup, self.runner.emit)
    }

    /// What an image built on `image` keeps of it: its build steps, including those that
    /// only change metadata, and its layer contents. Nothing build-specific is stamped on
    /// the images, so a cached rebuild of the same recipes reproduces the final digest.
    fn ancestry(&self, image: &str) -> Result<Ancestry> {
        let history = self.quiet(
            "local image history",
            (self.docker)().args([
                "history",
                "--no-trunc",
                "--human=false",
                "--format",
                HISTORY_FORMAT,
                image,
            ]),
        )?;
        let layers = self.quiet(
            "local image layers",
            (self.docker)().args(["image", "inspect", "--format", "{{json .RootFS.Layers}}", image]),
        )?;
        let ancestry = Ancestry {
            history: history.lines().map(str::to_owned).collect(),
            layers: serde_json::from_str(layers.trim()).map_err(|_| Error::Invalid("Invalid local image layers"))?,
        };
        if ancestry.history.is_empty() {
            return Err(Error::Invalid("A local image reports no build history"));
        }
        Ok(ancestry)
    }

    /// Runs `command` without echoing its output, which can be long and names build steps,
    /// and passes that output on, redacted as the build's, only when the command fails.
    fn quiet(&self, name: &'static str, command: &mut Command) -> Result<String> {
        let output = std::cell::RefCell::new(Vec::new());
        let collect = |event| output.borrow_mut().push(event);
        let result = Runner {
            cancel: self.runner.cancel,
            emit: &collect,
            secrets: self.runner.secrets.clone(),
        }
        .run(name, command, Duration::from_secs(30));
        if result.is_err() {
            output.into_inner().into_iter().for_each(self.runner.emit);
        }
        result
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

/// The build steps, newest first, and layer diff IDs, oldest first, of a local image.
struct Ancestry {
    history: Vec<String>,
    layers: Vec<String>,
}

impl Ancestry {
    /// Whether an image with this ancestry was built on `base`: it ends with the base's whole
    /// history and starts with its layers. A recipe that ignores `HORIZON_BASE` and builds on
    /// another image fails this, unless that image itself descends from an identical cached
    /// base, whose steps and layers the check cannot tell apart.
    fn descends_from(&self, base: &Self) -> bool {
        self.history.ends_with(&base.history) && self.layers.starts_with(&base.layers)
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
