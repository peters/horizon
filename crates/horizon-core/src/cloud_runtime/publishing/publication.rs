//! Retained nonsecret image destination and publisher identity.
use super::{Account, Destination, Error, Publisher, Result, Selection};
use horizon_cloud::Profile;
use serde::{Deserialize, Serialize};

/// A retry and a rebuild retain the original recipe, destination and account.
/// This record contains no credential and does not authorize a publication.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub profile: String,
    pub requested_image: String,
    pub destination: Destination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
}

impl Publication {
    pub(super) fn check_selection(&self, shown: &Selection) -> Result<()> {
        let matches = match shown {
            Selection::Personal { destination, account } => {
                self.destination == *destination && self.account.as_ref() == Some(account)
            }
            Selection::Registry { destination } => self.destination == *destination && self.account.is_none(),
            Selection::Automatic { login, repository } => {
                matches!(&self.destination.publisher, Publisher::GitHub { login: recorded }
                    if login.as_ref().is_none_or(|shown| shown.eq_ignore_ascii_case(recorded)))
                    && repository
                        .as_ref()
                        .is_none_or(|shown| shown == &self.destination.repository)
            }
            Selection::Recipe { .. } => false,
        };
        if !matches {
            return Err(Error::Invalid(
                "The cloud's recorded destination differs from the confirmed plan; review it before starting",
            ));
        }
        Ok(())
    }

    pub(in crate::cloud_runtime) fn validate(&self, profile: &Profile) -> Result<()> {
        self.destination
            .validate()
            .map_err(|_| Error::Invalid("Invalid recorded image destination"))?;
        if !horizon_cloud::valid_id(&self.profile)
            || (!self.requested_image.is_empty() && !horizon_cloud::valid_image(&self.requested_image))
            || self.destination.repository != profile.image
            || profile.build.is_none()
        {
            return Err(Error::Invalid(
                "The cloud's image destination differs from its publishing record",
            ));
        }
        match (&self.destination.publisher, &self.account) {
            (Publisher::GitHub { login }, Some(account)) if login == &account.login => account.check(account),
            (Publisher::Registry { .. }, None) => Ok(()),
            _ => Err(Error::Invalid(
                "The cloud's publishing account is missing or inconsistent",
            )),
        }
    }
}
