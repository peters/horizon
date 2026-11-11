#![forbid(unsafe_code)]

pub mod agent_work;
mod agents;
mod atomic_file;
mod block_panel;
mod board;
mod bounded_program;
pub mod browser;
mod config;
mod config_migration;
pub mod config_store;
mod device;
pub mod dir_search;
mod editor;
mod error;
mod horizon_home;
mod layout;
mod local_store;
pub mod machines;
pub mod named_secrets;
mod opencode_paths;
mod panel;
pub mod place;
pub mod qr;
pub mod remote_browser_credential;
mod remote_hosts;
mod retired_panel_kinds;
mod runtime_state;
pub mod search;
pub mod secret;
pub mod secret_store;
pub mod server_link;
mod session_store;
mod shortcuts;
pub mod speech;
mod ssh;
mod terminal;
mod transcript;
pub mod ui_thread;
mod util;
mod view;
mod workspace;
pub mod workspace_list;

pub use agents::{
    AgentDefinition, AgentIntegrationKind, AgentResumeMode, AgentSessionValidationMode, AgentStatus, agent_definition,
    all_agent_kinds, is_agent_working_line,
};
pub use alacritty_terminal::index::Side as TerminalSide;
pub use alacritty_terminal::selection::SelectionType;
pub use atomic_file::AtomicFile;
pub use block_panel::{
    BlockPanelState, CLICK_RETENTION_MILLIS, CardFrames, CardRuntime, ClickQueue, MAX_PENDING_CLICKS, PendingClick,
    WAKE_LINE, WAKE_LINE_INTERVAL, WakeContext, WakeHold, WakeVerdict, wake_verdict,
};
pub use board::{
    Board, EditorSave, ForcedBrowserShutdownStatus, OrphanedRemoteHold, ShutdownProgress, WorkspaceAlignment,
    WorkspaceDockSide, WorkspaceLayout,
};
pub use bounded_program::ProgramEnvironment;
pub use config::{
    AppearanceConfig, AppearanceTheme, BrowserHostConfig, Config, DEFAULT_LINK_COMMAND, DEFAULT_SPEECH_ENDPOINT,
    DEFAULT_SPEECH_HOTKEY, DEFAULT_SPEECH_KEY_SECRET, DEFAULT_SPEECH_MODEL, FeaturesConfig, MAX_LANGUAGE_CHARS,
    OverlaysConfig, PresetConfig, RemoteHostsConfig, ServerConfig, ShortcutsConfig, SpeechConfig, TerminalConfig,
    TranscriberConfig, TranscriberKind, WindowConfig, WorkspaceConfig, browser_host, browser_mcp_args,
    configure_browser_host, configure_server, language_hint, server,
};
pub use device::{DeviceInput, DevicePanelState, DeviceViewTarget};
pub use editor::{MarkdownEditor, PanelContent, PreviewMode};
pub use error::{Error, Result};
pub use horizon_home::{HorizonHome, HorizonHomeError, browser_mcp_executable};
pub use local_store::{codex_home_dir, grok_home_dir, user_home_dir};
pub use machines::{
    CheckedMachine, MAX_MACHINES, MAX_PROVIDER_CHARS, MAX_SESSION_CHARS, Machine, MachineBindings, MachineError,
    MachineRegistry, MachineStore, Machines, check_record,
};
pub use panel::{
    DEFAULT_PANEL_SIZE, PANEL_SCROLLBACK_LIMIT, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume,
    PanelStart, PendingRestart, RestartPreflight, RestartRequest, SshConnectionEnd, StartRequest, browser_actor,
    finish_restart, prepare_panel_start, prepare_restart,
};
pub use place::{
    AfterConnectionLoss, ConnectionLoss, PlaceError, PlaceWorkspace, RECONNECT_RESET_AFTER, SESSION_KILL_TIMEOUT,
    SSH_CONNECTION_FAILURE, SessionKill, WorkspacePlace, after_connection_loss, attaches_a_tmux_session, kill_command,
    moves_to_the_machine, place_command, reconnect_delay, session_command, session_prefix,
};
pub use qr::{QUIET_ZONE, Qr, QrError};
pub use remote_hosts::{
    RemoteHost, RemoteHostCatalog, RemoteHostConnectionHistoryEntry, RemoteHostConnectionSummary, RemoteHostSources,
    RemoteHostStatus, discover_remote_hosts, summarize_remote_host_connections,
};
pub use retired_panel_kinds::RetiredPanels;
pub use runtime_state::{
    AgentSessionBinding, AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionDeletionCleanupWarning,
    AgentSessionDeletionFailure, AgentSessionDeletionRecovery, AgentSessionDeletionReport,
    AgentSessionDeletionReservation, AgentSessionKey, AgentSessionRecord, BrowserProfileState, DetachedWorkspaceState,
    PanelState, PanelTemplateRef, RuntimeState, WorkspaceState, WorkspaceTemplateRef, live_claude_session_ids,
    new_local_id, reserve_saved_session_deletions, saved_session_deletion_pending,
};
pub use search::{PanelSearchResult, SearchMatch, SearchOptions, SearchResults, search_board};
pub use secret::{MachineKeyFiles, SecretError, SecretId, SecretKind, SecretTarget, SecretVault};
pub use server_link::{JoinLink, LinkError, ServerAddress, allows_address, check_address, check_code, normalize_code};
pub use session_store::{
    ResolvedSession, SessionDeletionNotice, SessionLease, SessionOpenDisposition, SessionStore, SessionSummary,
    StartupChooser, StartupDecision, StartupPromptReason,
};
pub use shortcuts::{AppShortcuts, ShortcutBinding, ShortcutKey, ShortcutModifiers};
pub use ssh::{
    DiscoveredSshHost, MachineKey, MachineKeyError, MachineKeyRef, MachineKeys, SshConnection, SshConnectionStatus,
    discover_ssh_hosts,
};
pub use terminal::{ClipboardTarget, ClipboardWrite, CwdReader, PROGRAM_BOUND, Terminal, open_url, read_cwd};
pub use transcript::PanelTranscript;
pub use transcript::warm_script_support;
pub use util::{stand_in_program, stand_in_program_in, truncate_chars};
pub use view::{CanvasViewState, DEFAULT_CANVAS_ZOOM, MAX_CANVAS_ZOOM, MIN_CANVAS_ZOOM, clamp_canvas_zoom};
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
