//! The state of the agents of one machine over a sequence of reports: a turn, an
//! approval, a first sight in the middle of a turn, a look and a pane that leaves.

use crate::PanelKind;
use crate::agent_work::TitleState;
use crate::machine_watch::{AgentObservation, AgentState, MachineModel};

/// One report of the agent of the pane `%0` of the session `hz-1`.
fn one(title: TitleState, working: bool) -> Vec<AgentObservation> {
    vec![AgentObservation {
        pane: "%0".to_owned(),
        session: "hz-1".to_owned(),
        kind: PanelKind::Codex,
        name: "shop".to_owned(),
        title,
        working,
    }]
}

/// The state of the only row of `model`.
fn state(model: &MachineModel) -> AgentState {
    let rows = model.rows();
    assert_eq!(rows.len(), 1, "one row of the model: {rows:?}");
    rows[0].state
}

#[test]
fn a_turn_of_an_agent_that_the_person_did_not_see_is_done() {
    let mut model = MachineModel::new("build-1");
    assert!(
        model.observe(&one(TitleState::Plain, false)),
        "the first row is a change"
    );
    assert_eq!(
        state(&model),
        AgentState::Idle,
        "the first sight while idle claims nothing"
    );
    assert!(model.observe(&one(TitleState::Working, false)));
    assert_eq!(state(&model), AgentState::Working);
    assert!(model.observe(&one(TitleState::Plain, false)));
    assert_eq!(
        state(&model),
        AgentState::Done,
        "the turn ended while the person did not look"
    );
    assert!(
        !model.observe(&one(TitleState::Plain, false)),
        "a repeated idle report of a finished turn changes no row"
    );
    assert_eq!(state(&model), AgentState::Done, "the row stays until the person looks");
}

#[test]
fn a_finished_turn_is_unread_also_when_the_person_looked_while_it_ran() {
    let mut model = MachineModel::new("build-1");
    model.observe(&one(TitleState::Working, false));
    assert!(
        !model.mark_seen("hz-1", "%0"),
        "the report held no finished turn to clear"
    );
    model.observe(&one(TitleState::Plain, false));
    assert_eq!(
        state(&model),
        AgentState::Done,
        "the turn ended after the look, so its row waits for the next one"
    );
    assert!(model.mark_seen("hz-1", "%0"), "the look clears the finished turn");
    assert_eq!(state(&model), AgentState::Idle);
    assert!(
        !model.observe(&one(TitleState::Plain, false)),
        "an idle report after the look changes no row"
    );
    assert_eq!(state(&model), AgentState::Idle);
    assert!(!model.mark_seen("hz-1", "%0"), "a second look changes nothing");
    assert!(!model.mark_seen("hz-1", "%9"), "an unknown pane changes nothing");

    model.observe(&one(TitleState::Working, false));
    model.observe(&one(TitleState::Plain, false));
    assert_eq!(
        state(&model),
        AgentState::Done,
        "a later turn that ends unseen is unread"
    );
}

#[test]
fn a_first_sight_in_the_middle_of_a_turn_is_working_and_its_end_is_done() {
    let mut model = MachineModel::new("build-1");
    model.observe(&one(TitleState::Working, false));
    assert_eq!(
        state(&model),
        AgentState::Working,
        "the watch may start in the middle of a turn of another person"
    );
    model.observe(&one(TitleState::Plain, false));
    assert_eq!(state(&model), AgentState::Done);
}

#[test]
fn the_title_of_a_waiting_agent_needs_the_person() {
    let mut model = MachineModel::new("build-1");
    model.observe(&one(TitleState::Working, false));
    model.observe(&one(TitleState::NeedsYou, false));
    assert_eq!(state(&model), AgentState::NeedsYou);
    assert_eq!(AgentState::NeedsYou.word(), Some("Needs you"));
    assert_eq!(
        model.mark_seen("hz-1", "%0"),
        false,
        "a look does not answer the question of the agent"
    );
    assert_eq!(state(&model), AgentState::NeedsYou);
    model.observe(&one(TitleState::Plain, false));
    assert_eq!(
        state(&model),
        AgentState::Done,
        "the wait ended and the turn of the person is unread"
    );
}

#[test]
fn a_working_line_of_the_screen_answers_when_the_title_names_no_state() {
    let mut model = MachineModel::new("build-1");
    model.observe(&one(TitleState::Plain, true));
    assert_eq!(
        state(&model),
        AgentState::Working,
        "the screen of a kind without a title rule"
    );
    model.observe(&one(TitleState::Plain, false));
    assert_eq!(state(&model), AgentState::Done);
    model.mark_seen("hz-1", "%0");
    model.observe(&one(TitleState::Working, false));
    assert_eq!(
        state(&model),
        AgentState::Working,
        "a title that names a state answers before the screen"
    );
}

#[test]
fn a_pane_that_leaves_drops_its_row_and_comes_back_as_a_first_sight() {
    let mut model = MachineModel::new("build-1");
    model.observe(&one(TitleState::Working, false));
    assert!(model.rows()[0].machine == "build-1");
    assert!(model.observe(&[]), "a report without the pane is a change");
    assert!(model.rows().is_empty(), "the row left with its pane");
    assert!(!model.observe(&[]), "an empty report of an empty model changes nothing");
    model.observe(&one(TitleState::Plain, true));
    assert_eq!(
        state(&model),
        AgentState::Working,
        "the pane came back with a turn that runs"
    );
    assert!(model.clear());
    assert!(!model.clear(), "the second clear changes nothing");
    assert!(model.rows().is_empty());
}

#[test]
fn the_state_of_a_row_names_its_weight_and_its_word() {
    assert_eq!(AgentState::default(), AgentState::Idle);
    assert_eq!(AgentState::Idle.word(), None);
    assert_eq!(AgentState::Working.word(), Some("Working"));
    assert_eq!(AgentState::Done.word(), Some("Done"));
    assert!(AgentState::NeedsYou.rank() > AgentState::Done.rank());
    assert!(AgentState::Done.rank() > AgentState::Working.rank());
    assert!(AgentState::Working.rank() > AgentState::Idle.rank());
    assert!(AgentState::Done.waits());
    assert!(!AgentState::Working.waits());
}

#[test]
fn a_pane_that_runs_another_kind_and_another_folder_updates_its_row() {
    let mut model = MachineModel::new("build-1");
    let mut agent = one(TitleState::Plain, false);
    model.observe(&agent);
    agent[0].kind = PanelKind::Claude;
    agent[0].name = "other".to_owned();
    assert!(model.observe(&agent), "the row changed");
    let rows = model.rows();
    assert_eq!(rows[0].kind, PanelKind::Claude);
    assert_eq!(rows[0].name, "other");
    assert_eq!(rows[0].session, "hz-1");
    assert_eq!(rows[0].pane, "%0");
}
