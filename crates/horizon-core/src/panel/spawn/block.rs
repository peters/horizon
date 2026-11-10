use crate::{BlockPanelState, editor::PanelContent};

use super::{Panel, PanelKind, StaticPanelSeed};

pub(super) fn spawn_block(mut seed: StaticPanelSeed, mut state: BlockPanelState) -> Panel {
    let (title, has_custom_name) = seed.take_title(|| BlockPanelState::DEFAULT_TITLE.to_owned());
    state.title = BlockPanelState::normalize_title(&title);
    let title = state.title.clone();
    tracing::info!("created card panel '{}' (id={})", title, seed.id.0);

    seed.into_panel(
        title,
        PanelKind::Block,
        PanelContent::Block(state),
        None,
        None,
        has_custom_name,
    )
}
