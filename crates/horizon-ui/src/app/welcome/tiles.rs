//! The four starts of the first page, and every word of them.
//!
//! The words are the approved mockup of the first start. Each tile names one
//! thing that a person can do with an empty Horizon, says what it gives them, and
//! names the press that starts it.

use horizon_core::SetupGoal;

/// One of the four tiles of the first page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum WelcomeTile {
    /// Run agents side by side on this computer.
    Agents,
    /// Rent a cloud machine in the person's own account.
    Machine,
    /// One Horizon server for all of the person's computers.
    Server,
    /// A browser that stays signed in.
    Browser,
}

impl WelcomeTile {
    /// Every tile, in the order of the page.
    pub(in crate::app) const ALL: [Self; 4] = [Self::Agents, Self::Machine, Self::Server, Self::Browser];

    /// The title of the tile.
    pub(in crate::app) const fn title(self) -> &'static str {
        match self {
            Self::Agents => "Run agents side by side",
            Self::Machine => "Work on a bigger machine",
            Self::Server => "Use Horizon on all your computers",
            Self::Browser => "A browser that stays signed in",
        }
    }

    /// What the title gives the person.
    pub(in crate::app) const fn sentence(self) -> &'static str {
        match self {
            Self::Agents => {
                "Each agent has its own panel on the canvas. The sidebar shows which one waits for you, so you do not \
                 watch them."
            }
            Self::Machine => {
                "Rent a cloud machine in your own account and work on it as if it were here. You pay the provider, and \
                 Delete ends the cost."
            }
            Self::Server => {
                "One server of your own keeps your machines, keys and settings. Change computer, and the same list is \
                 there."
            }
            Self::Browser => {
                "Sign in one time on a machine that stays on. You and your agents use the same pages from any computer."
            }
        }
    }

    /// The press that starts the tile, as the last line of the tile says it.
    pub(in crate::app) const fn action(self) -> &'static str {
        match self {
            Self::Agents => "Start with one agent",
            Self::Machine => "Rent a machine",
            Self::Server => "Set up the server",
            Self::Browser => "Set up a browser",
        }
    }

    /// The goal that Set up receives when this tile is pressed, or `None` for
    /// the tile that starts an agent panel on this computer.
    pub(in crate::app) const fn goal(self) -> Option<SetupGoal> {
        match self {
            Self::Agents => None,
            Self::Machine => Some(SetupGoal::Rent),
            Self::Server => Some(SetupGoal::Server),
            Self::Browser => Some(SetupGoal::Browser),
        }
    }

    /// What a screen reader hears for this tile: every word that it shows.
    pub(in crate::app) fn spoken(self) -> String {
        format!("{}. {}. {}.", self.title(), self.sentence(), self.action())
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::SetupGoal;

    use super::WelcomeTile;

    #[test]
    fn the_page_names_four_starts_and_the_three_goals_of_set_up() {
        let goals: Vec<Option<SetupGoal>> = WelcomeTile::ALL.iter().map(|tile| tile.goal()).collect();
        assert_eq!(
            goals,
            vec![
                None,
                Some(SetupGoal::Rent),
                Some(SetupGoal::Server),
                Some(SetupGoal::Browser),
            ]
        );
        assert_eq!(WelcomeTile::ALL.len(), 4, "four tiles fill one row of the wide page");
    }

    #[test]
    fn every_tile_says_what_it_gives_and_what_the_press_starts() {
        for tile in WelcomeTile::ALL {
            let spoken = tile.spoken();
            assert!(spoken.starts_with(tile.title()), "{spoken}");
            assert!(spoken.contains(tile.sentence()), "{spoken}");
            assert!(spoken.ends_with(&format!("{}.", tile.action())), "{spoken}");
            assert!(!tile.title().is_empty() && !tile.action().is_empty());
        }
    }

    #[test]
    fn the_four_titles_are_the_four_of_the_mockup() {
        let titles: Vec<&str> = WelcomeTile::ALL.iter().map(|tile| tile.title()).collect();
        assert_eq!(
            titles,
            vec![
                "Run agents side by side",
                "Work on a bigger machine",
                "Use Horizon on all your computers",
                "A browser that stays signed in",
            ]
        );
    }
}
