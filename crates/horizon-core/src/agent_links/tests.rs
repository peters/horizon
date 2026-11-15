use super::ACTOR_PREFIX;
use crate::{
    AgentLink, BlockPanelState, Board, PanelId, PanelKind, PanelOptions, SshConnection, WorkspaceId, browser_actor,
};

/// An agent panel of `workspace`. An agent panel waits for the preparation of
/// its process, so the test starts none.
fn agent(board: &mut Board, workspace: WorkspaceId, kind: PanelKind) -> PanelId {
    let options = PanelOptions {
        kind,
        ..PanelOptions::default()
    };
    board.create_panel_waiting(options, workspace).expect("an agent panel")
}

/// The `horizon:<local_id>` actor of the agent panel `id`, as a card or a
/// browser claim names it.
fn actor_of(board: &Board, id: PanelId) -> String {
    let local_id = board.panel(id).map(|panel| panel.local_id.clone()).expect("the panel");
    browser_actor(&local_id)
}

/// A card of `workspace` whose owner is `owner`, as the block tool records it.
fn card(board: &mut Board, workspace: WorkspaceId, owner: Option<&str>) -> PanelId {
    let state = BlockPanelState {
        owner_actor: owner.map(str::to_owned),
        ..BlockPanelState::new(BlockPanelState::empty_document())
    };
    let options = PanelOptions {
        kind: PanelKind::Block,
        block: Some(state),
        ..PanelOptions::default()
    };
    board.create_panel_waiting(options, workspace).expect("a card")
}

/// An SSH terminal that an agent of kind `mark` opened for the person.
fn opened_terminal(board: &mut Board, workspace: WorkspaceId, mark: &str) -> PanelId {
    let options = PanelOptions {
        kind: PanelKind::Ssh,
        opened_by: Some(mark.to_owned()),
        ssh_connection: Some(SshConnection {
            host: "192.0.2.10".to_owned(),
            ..SshConnection::default()
        }),
        ..PanelOptions::default()
    };
    board
        .create_panel_waiting(options, workspace)
        .expect("a terminal that an agent opened")
}

/// An SSH terminal that the agent panel `opener` opened for the person, as the
/// terminal tool records it: the mark of the kind and the actor of that panel.
fn opened_terminal_of(board: &mut Board, workspace: WorkspaceId, opener: PanelId) -> PanelId {
    let mark = board
        .panel(opener)
        .map(|panel| panel.kind.display_name().to_owned())
        .expect("the agent panel");
    let actor = actor_of(board, opener);
    let options = PanelOptions {
        kind: PanelKind::Ssh,
        opened_by: Some(mark),
        opened_by_actor: Some(actor),
        ssh_connection: Some(SshConnection {
            host: "192.0.2.12".to_owned(),
            ..SshConnection::default()
        }),
        ..PanelOptions::default()
    };
    board
        .create_panel_waiting(options, workspace)
        .expect("a terminal that an agent opened")
}

/// A terminal that the person opened themselves.
fn person_terminal(board: &mut Board, workspace: WorkspaceId) -> PanelId {
    let options = PanelOptions {
        kind: PanelKind::Ssh,
        ssh_connection: Some(SshConnection {
            host: "192.0.2.11".to_owned(),
            ..SshConnection::default()
        }),
        ..PanelOptions::default()
    };
    board.create_panel_waiting(options, workspace).expect("a terminal")
}

/// A page whose browser program never starts, so the panel holds the state of a
/// browser panel without a browser.
fn browser_panel(board: &mut Board, workspace: WorkspaceId) -> PanelId {
    let root = tempfile::tempdir().expect("a folder for the profile of the test");
    let options = PanelOptions {
        kind: PanelKind::Browser,
        command: Some("https://example.test/".to_owned()),
        browser_config: Some(crate::browser::BrowserConfig {
            command: Some(root.path().join("no-browser").to_string_lossy().into_owned()),
            profile_root: Some(root.path().to_path_buf()),
            ..crate::browser::BrowserConfig::default()
        }),
        ..PanelOptions::default()
    };
    board.create_panel_waiting(options, workspace).expect("a browser panel")
}

/// The pairs of `board` as `(agent, target)`, in a fixed order.
fn pairs(board: &Board) -> Vec<(PanelId, PanelId)> {
    let mut pairs: Vec<(PanelId, PanelId)> = board
        .agent_links()
        .iter()
        .map(|link| (link.agent, link.target))
        .collect();
    pairs.sort_by_key(|(agent, target)| (agent.0, target.0));
    pairs
}

/// A card that an agent owns, a terminal that an agent opened for the person and
/// a panel of the person: one pair for each fact that names an agent, and no pair
/// for the panel that names none and for the card whose owner is gone.
#[test]
fn a_controlled_panel_pairs_with_the_agent_that_names_it() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let claude = agent(&mut board, workspace, PanelKind::Claude);
    let actor = actor_of(&board, claude);
    let owned = card(&mut board, workspace, Some(&actor));
    let gone = card(&mut board, workspace, Some(&browser_actor("a-panel-that-closed")));
    let opened = opened_terminal(&mut board, workspace, "Claude");
    let person = person_terminal(&mut board, workspace);

    assert_eq!(
        pairs(&board),
        vec![(claude, owned), (claude, opened)],
        "the card that the agent owns and the terminal that it opened pair with it, while the \
         card of a gone owner and the panel of the person pair with no one"
    );
    assert_ne!(gone, owned);
    assert_ne!(person, opened);
}

/// A helper agent that an agent opened has a pair to its opener, and it is the
/// agent end of the pair of the card that it owns: three pairs with the chain.
#[test]
fn a_helper_agent_pairs_with_its_opener_and_controls_its_own_card() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let claude = agent(&mut board, workspace, PanelKind::Claude);
    let helper = agent(&mut board, workspace, PanelKind::Codex);
    if let Some(panel) = board.panels.iter_mut().find(|panel| panel.id == helper) {
        panel.opened_by = Some("Claude".to_owned());
    }
    let helper_actor = actor_of(&board, helper);
    let helper_card = card(&mut board, workspace, Some(&helper_actor));
    let claude_terminal = opened_terminal(&mut board, workspace, "Claude");

    assert_eq!(
        pairs(&board),
        vec![(claude, helper), (claude, claude_terminal), (helper, helper_card)],
        "the helper pairs with its opener and is the agent end of its own card"
    );
}

/// The mark of an opened panel names a kind, so it names one panel only when one
/// agent panel of that kind stands in the workspace of that panel.
#[test]
fn two_agents_of_one_kind_leave_the_mark_of_an_opened_panel_unresolved() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    agent(&mut board, workspace, PanelKind::Claude);
    agent(&mut board, workspace, PanelKind::Claude);
    opened_terminal(&mut board, workspace, "Claude");

    assert!(
        pairs(&board).is_empty(),
        "one mark for two agents of one kind names neither of them, so nothing is drawn"
    );

    // An agent of that kind in another workspace changes nothing: the mark names
    // an agent of the workspace of the panel that it opened.
    let other = board.create_workspace("Other");
    let third = agent(&mut board, other, PanelKind::Claude);
    opened_terminal(&mut board, other, "Claude");
    assert_eq!(
        pairs(&board),
        vec![(
            third,
            board.panels.last().expect("the terminal of the other workspace").id
        )],
        "the mark of the other workspace names its one agent of that kind"
    );
}

/// The actor of an opened panel names one agent panel also when two of them have
/// one kind: each terminal pairs with the panel that opened it.
#[test]
fn two_agents_of_one_kind_each_pair_with_the_terminal_that_it_opened() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let first = agent(&mut board, workspace, PanelKind::Claude);
    let second = agent(&mut board, workspace, PanelKind::Claude);
    let first_terminal = opened_terminal_of(&mut board, workspace, first);
    let second_terminal = opened_terminal_of(&mut board, workspace, second);

    assert_eq!(
        pairs(&board),
        vec![(first, first_terminal), (second, second_terminal)],
        "the actor of each opener names its own panel"
    );
}

/// A terminal of a session of an older build holds no actor, and the mark of its
/// kind keeps the answer that it gave before.
#[test]
fn a_terminal_of_an_older_build_still_pairs_by_its_mark() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let claude = agent(&mut board, workspace, PanelKind::Claude);
    let terminal = opened_terminal(&mut board, workspace, "Claude");

    assert_eq!(pairs(&board), vec![(claude, terminal)]);

    // A second agent of that kind leaves the mark of the older terminal
    // unresolved, exactly as it did before an actor was saved.
    agent(&mut board, workspace, PanelKind::Claude);
    assert!(pairs(&board).is_empty());
}

/// The place of an agent among the agent panels of its workspace is the accent
/// index that gives each agent of a workspace its own colour.
#[test]
fn the_accent_of_an_agent_follows_the_order_of_its_workspace() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let other = board.create_workspace("Other");
    let first = agent(&mut board, workspace, PanelKind::Claude);
    let second = agent(&mut board, workspace, PanelKind::Codex);
    let alone = agent(&mut board, other, PanelKind::Pi);
    let first_terminal = opened_terminal(&mut board, workspace, "Claude");
    let second_terminal = opened_terminal(&mut board, workspace, "Codex");
    let alone_terminal = opened_terminal(&mut board, other, "Pi");

    let links: Vec<AgentLink> = board.agent_links();
    let link = |agent: PanelId, target: PanelId| {
        links
            .iter()
            .find(|link| link.agent == agent && link.target == target)
            .copied()
            .expect("the pair of this panel")
    };

    assert_eq!(
        link(first, first_terminal).accent,
        0,
        "the first agent of its workspace"
    );
    assert_eq!(
        link(second, second_terminal).accent,
        1,
        "the second agent of its workspace"
    );
    assert_eq!(
        link(alone, alone_terminal).accent,
        0,
        "the first agent of another workspace"
    );
}

/// A board whose panels name no agent answers with no pair, and the cheap check
/// that a frame makes first agrees with it.
#[test]
fn a_board_without_an_owner_fact_has_no_pair() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    person_terminal(&mut board, workspace);
    card(&mut board, workspace, None);
    agent(&mut board, workspace, PanelKind::Claude);

    assert!(board.agent_links().is_empty());
    assert!(!board.has_agent_links(), "no panel of this board carries an owner fact");

    let terminal = opened_terminal(&mut board, workspace, "Claude");
    assert!(board.has_agent_links(), "the mark of an opened panel is an owner fact");
    assert_eq!(board.agent_links()[0].target, terminal);
}

/// The prefix of this module is the prefix of the actor of a panel.
#[test]
fn the_prefix_of_an_actor_is_the_prefix_that_horizon_writes() {
    assert_eq!(browser_actor("panel-1"), format!("{ACTOR_PREFIX}panel-1"));
    assert_eq!(browser_actor(""), browser_actor(""));
    assert!(
        browser_actor("").starts_with(ACTOR_PREFIX),
        "even a hashed id takes the prefix"
    );
}

/// A page names the agent that drives it through the live claim of that agent.
#[test]
fn a_browser_panel_pairs_with_the_agent_that_claims_it() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let claude = agent(&mut board, workspace, PanelKind::Claude);
    let actor = actor_of(&board, claude);
    let browser = browser_panel(&mut board, workspace);

    assert!(
        board.agent_links().is_empty(),
        "a page that no agent drives pairs with no one"
    );

    if let Some(state) = board
        .panels
        .iter_mut()
        .find(|panel| panel.id == browser)
        .and_then(|panel| panel.browser_mut())
    {
        state.owner = Some(actor);
    }

    assert_eq!(pairs(&board), vec![(claude, browser)]);
}
