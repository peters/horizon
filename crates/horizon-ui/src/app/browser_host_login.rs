//! The login of the machine that the browser host of this run names.
//!
//! A machine that Horizon rented holds the public key of Horizon and no key of
//! the person, so its browser answers to the machine key of this computer alone.
//! A browser host entry that names such a machine is therefore resolved here, from
//! the record of the machine list and the key that this computer binds to it,
//! exactly as the server link resolves one for a server that runs on a machine.
//!
//! The record is what this computer already holds in memory and the key binding is
//! a name that resolves to the key folder, so this reads no file and runs on the
//! frame that creates the panel. Every panel of a person and every panel of an
//! agent takes this one path, so no create starts an `ssh` that would ask a machine
//! of the list for a key that it does not have.

use horizon_core::{BrowserHostConfig, MachineKeys, PanelKind, PanelOptions, SshConnection};

use super::HorizonApp;

impl HorizonApp {
    /// Gives a browser panel that follows the browser host of this run the login
    /// of the machine that the entry names.
    ///
    /// A panel of another kind, a panel that names its own target, and a browser
    /// host of the SSH configuration of the person change nothing.
    ///
    /// # Errors
    /// One sentence when the entry names a machine that the machine list of this
    /// computer does not hold, or whose key this computer does not hold: a create
    /// that says why is better than a panel that fails later without the reason.
    pub(in crate::app) fn resolve_browser_host_login(&self, options: &mut PanelOptions) -> Result<(), String> {
        match horizon_core::browser_host() {
            Some(browser_host) => self.apply_browser_host_login(browser_host, options),
            None => Ok(()),
        }
    }

    /// [`Self::resolve_browser_host_login`] for one entry, which is a parameter so
    /// that a test reads no process-wide setting.
    fn apply_browser_host_login(
        &self,
        browser_host: &BrowserHostConfig,
        options: &mut PanelOptions,
    ) -> Result<(), String> {
        if options.kind != PanelKind::Browser || !options.follows_browser_host || options.browser_host.is_some() {
            return Ok(());
        }
        let Some(machine) = browser_host.machine() else {
            return Ok(());
        };
        let keys = MachineKeys::from_home(self.session_store.home());
        let login: SshConnection =
            horizon_core::machines::login_of(&self.machine_list, &keys, machine).map_err(|error| error.to_string())?;
        tracing::info!(
            machine,
            target = login.transport_target(),
            "a browser panel of a machine of the list takes the login of that machine"
        );
        options.ssh_connection = Some(login);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::machines::{MachineRegistry, Machines};
    use horizon_core::{Machine, PanelOptions};

    use super::*;
    use crate::app::test_support::test_app;

    /// A browser panel that follows the browser host of this run.
    fn following_browser() -> PanelOptions {
        PanelOptions {
            kind: PanelKind::Browser,
            follows_browser_host: true,
            ..PanelOptions::default()
        }
    }

    fn on_machine(machine: &str) -> BrowserHostConfig {
        BrowserHostConfig {
            machine: Some(machine.to_owned()),
            profile: Some("work".to_owned()),
            ..BrowserHostConfig::default()
        }
    }

    /// Writes the record of `name` and the key that this computer binds to it into
    /// the machine list of `app`.
    fn plant_machine(app: &mut HorizonApp, name: &str) {
        let registry = MachineRegistry::from_home(app.session_store.home());
        let list = Machines::load(registry.clone()).expect("the machine list of this computer");
        let (list, _) = list
            .with_record(
                Machine {
                    name: name.to_owned(),
                    host: "10.0.0.5".to_owned(),
                    user: Some("peters".to_owned()),
                    ..Machine::default()
                },
                Some(name),
            )
            .expect("a new record");
        list.save().expect("the record is written");
        app.machine_list = Machines::load(registry).expect("the machine list of this computer");
        let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
        std::fs::create_dir_all(&directory).expect("the key directory");
        std::fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the key");
    }

    #[test]
    fn a_browser_panel_of_a_machine_of_the_list_takes_the_login_of_that_machine() {
        let (_temp, mut app) = test_app();
        plant_machine(&mut app, "helsinki-1");
        let mut options = following_browser();
        app.apply_browser_host_login(&on_machine("helsinki-1"), &mut options)
            .expect("the login of the machine");
        let login = options.ssh_connection.expect("the login of the machine");
        assert_eq!(
            login.transport_target(),
            "peters@10.0.0.5",
            "the panel reaches the machine at its address with its user"
        );
        assert!(
            login.login_option_args().iter().any(|option| option == "-i"),
            "the machine key of this computer is the login: {:?}",
            login.login_option_args()
        );
    }

    #[test]
    fn a_browser_host_of_the_person_changes_no_panel_and_a_machine_that_is_gone_says_why() {
        let (_temp, mut app) = test_app();
        plant_machine(&mut app, "helsinki-1");
        let mut options = following_browser();
        let target = BrowserHostConfig {
            ssh_target: Some("browser-vm".to_owned()),
            profile: Some("work".to_owned()),
            ..BrowserHostConfig::default()
        };
        app.apply_browser_host_login(&target, &mut options)
            .expect("a host of the person's configuration needs no login");
        assert!(
            options.ssh_connection.is_none(),
            "the SSH configuration of the person stays the route of such a host"
        );

        // A machine that the list does not hold is a create that says why.
        let error = app
            .apply_browser_host_login(&on_machine("gone-2"), &mut options)
            .expect_err("a machine that the list does not hold");
        assert!(error.contains("no machine gone-2"), "{error}");

        // A panel of another kind is never routed by this entry.
        let mut terminal = PanelOptions {
            kind: PanelKind::Shell,
            follows_browser_host: true,
            ..PanelOptions::default()
        };
        app.apply_browser_host_login(&on_machine("gone-2"), &mut terminal)
            .expect("a terminal of this computer takes no browser host");
        assert!(terminal.ssh_connection.is_none());
    }
}
