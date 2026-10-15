//! Companion inventory reads only the host's nonsecret background snapshots.
use crate::app::tailnets::State;
use horizon_core::cloud_runtime::tailnet::{ApiCredentialObservation, Catalog, Tailnet};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(super) struct Inventory<'a> {
    pub(super) tailnets: Option<&'a [Tailnet]>,
    pub(super) api_credentials: BTreeMap<&'a str, ApiCredentialObservation>,
}

pub(super) fn inventory<'a>(state: &'a State, root: Option<&Path>) -> Inventory<'a> {
    snapshot(root.and_then(|root| state.cached_catalog_at(root)), |id| {
        state.api_credential_observation(id)
    })
}

fn snapshot(catalog: Option<&Catalog>, mut observation: impl FnMut(&str) -> ApiCredentialObservation) -> Inventory<'_> {
    let tailnets = catalog.map(|catalog| catalog.tailnets.as_slice());
    let api_credentials = tailnets
        .into_iter()
        .flatten()
        .map(|network| (network.id.as_str(), observation(&network.id)))
        .collect();
    Inventory {
        tailnets,
        api_credentials,
    }
}

impl super::HorizonApp {
    pub(in crate::app) fn answer_tailnet_credential(
        &mut self,
        request: &horizon_core::browser::manifest::provider_usage::UsageRequest,
    ) {
        use horizon_core::browser::manifest;
        let answer = (|| -> Result<Value, String> {
            if request.host_instance != manifest::host_instance()
                || crate::app::browser_requests::actor_panel(&self.board, &request.actor).is_none()
                || manifest::now_millis() >= request.deadline_at_millis
            {
                return Err("tailnet_credential_unavailable: caller or request expired".into());
            }
            let input = request
                .tailnet_credential
                .as_ref()
                .filter(|input| input.valid())
                .ok_or("tailnet_credential_invalid_request")?;
            let root = self
                .cloud_prototype
                .root
                .as_ref()
                .ok_or("tailnet_credential_unavailable")?;
            if matches!(
                input.operation,
                manifest::provider_usage::CredentialOperation::Status
                    | manifest::provider_usage::CredentialOperation::Cancel
            ) {
                return self
                    .browser_create_host
                    .provider_usage
                    .tailnet_credentials
                    .answer_existing(&request.actor, input, root, manifest::now_millis())
                    .map_err(str::to_string);
            }
            let network = self
                .cloud_prototype
                .production
                .tailnets
                .cached_catalog_at(root)
                .and_then(|catalog| catalog.tailnets.iter().find(|network| network.id == input.tailnet_id))
                .ok_or("tailnet_credential_unknown_tailnet: refresh cloud_companions")?;
            self.browser_create_host
                .provider_usage
                .tailnet_credentials
                .answer(
                    &request.actor,
                    input,
                    crate::app::tailnets::requests::Target {
                        root: root.clone(),
                        id: network.id.clone(),
                        name: network.name.clone(),
                    },
                    manifest::now_millis(),
                )
                .map_err(str::to_string)
        })();
        let mut result = request.result(Vec::new(), None);
        match answer {
            Ok(value) => result.tailnet_credential = Some(value),
            Err(error) => result.error = Some(error),
        }
        self.publish_tailnet_credential_result(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::tailnet::{ApiCredentialKind, ApiCredentialStatus};
    use serde_json::json;

    #[test]
    fn inventory_keeps_id_name_items_and_distinguishes_nonsecret_observations() {
        let catalog = Catalog {
            tailnets: ["pending", "unavailable", "missing", "personal", "oauth"]
                .into_iter()
                .map(|id| Tailnet {
                    id: id.into(),
                    name: format!("Network {id}"),
                })
                .collect(),
        };
        let inventory = snapshot(Some(&catalog), |id| match id {
            "pending" => ApiCredentialObservation::Pending,
            "unavailable" => ApiCredentialObservation::Unavailable,
            "missing" => ApiCredentialObservation::Ready(ApiCredentialStatus::Missing),
            "personal" => {
                ApiCredentialObservation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken))
            }
            "oauth" => ApiCredentialObservation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)),
            _ => panic!("only saved IDs are observed"),
        });
        let encoded = json!({"tailnets": inventory.tailnets, "tailnet_api_credentials": inventory.api_credentials});
        assert_eq!(encoded["tailnets"][0], json!({"id":"pending","name":"Network pending"}));
        assert_eq!(
            encoded["tailnet_api_credentials"]["pending"],
            json!({"status":"pending"})
        );
        assert_eq!(
            encoded["tailnet_api_credentials"]["unavailable"],
            json!({"status":"unavailable"})
        );
        assert_eq!(
            encoded["tailnet_api_credentials"]["missing"],
            json!({"status":"ready","credential":{"state":"missing"}})
        );
        assert_eq!(
            encoded["tailnet_api_credentials"]["personal"],
            json!({"status":"ready","credential":{"state":"stored","kind":"personal_token"}})
        );
        assert_eq!(
            encoded["tailnet_api_credentials"]["oauth"],
            json!({"status":"ready","credential":{"state":"stored","kind":"oauth"}})
        );
        for value in encoded["tailnet_api_credentials"].as_object().unwrap().values() {
            let observation: ApiCredentialObservation = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(observation).unwrap(), *value);
            assert!(!value.to_string().contains("secret") && !value.to_string().contains("token\":"));
        }
    }

    #[test]
    fn unavailable_catalog_does_not_probe_secrets_or_invent_saved_networks() {
        let inventory = snapshot(None, |_| panic!("no catalog means no credential observation"));
        assert!(inventory.tailnets.is_none());
        assert!(inventory.api_credentials.is_empty());
    }

    type StatusReply = Result<Vec<(String, Result<ApiCredentialStatus, String>)>, String>;

    fn settings_fixture(root: &Path) -> (State, std::sync::mpsc::Sender<StatusReply>) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let catalog = Catalog {
            tailnets: vec![Tailnet {
                id: "synthetic".into(),
                name: "Synthetic".into(),
            }],
        };
        (State::api_reply_fixture(root.into(), catalog, receiver), sender)
    }

    #[test]
    fn settings_completion_updates_mcp_before_settings_closes() {
        for status in [
            ApiCredentialStatus::Stored(ApiCredentialKind::OAuth),
            ApiCredentialStatus::Missing,
        ] {
            let root = Path::new("synthetic-shared-root");
            let ctx = egui::Context::default();
            let mut production = State::for_root_fixture(root.into());
            let (mut settings, sender) = settings_fixture(root);
            production.observe_settings(&mut settings, &ctx);
            assert_eq!(
                inventory(&production, Some(root)).api_credentials["synthetic"],
                ApiCredentialObservation::Pending
            );
            sender.send(Ok(vec![("synthetic".into(), Ok(status))])).unwrap();
            production.observe_settings(&mut settings, &ctx);
            let report = inventory(&production, Some(root));
            assert_eq!(
                report.api_credentials["synthetic"],
                ApiCredentialObservation::Ready(status)
            );
            assert_eq!(report.tailnets.unwrap()[0].id, "synthetic");
            assert!(
                inventory(&production, Some(Path::new("another-root")))
                    .tailnets
                    .is_none()
            );
        }
    }

    #[test]
    fn settings_failure_and_disconnect_are_unavailable_in_mcp() {
        for disconnect in [false, true] {
            let root = Path::new("synthetic-shared-root");
            let ctx = egui::Context::default();
            let mut production = State::for_root_fixture(root.into());
            let (mut settings, sender) = settings_fixture(root);
            production.observe_settings(&mut settings, &ctx);
            assert_eq!(
                inventory(&production, Some(root)).api_credentials["synthetic"],
                ApiCredentialObservation::Pending
            );
            if !disconnect {
                sender.send(Err("Synthetic locked store".into())).unwrap();
            }
            drop(sender);
            production.observe_settings(&mut settings, &ctx);
            assert_eq!(
                inventory(&production, Some(root)).api_credentials["synthetic"],
                ApiCredentialObservation::Unavailable
            );
        }
    }
}
