use std::sync::Arc;

use anyhow::Result;
use assets::Assets;
use gpui::{App, AppContext, KeyBinding, Menu, MenuItem, actions};
use gpui_component::input::{Copy, Cut, Paste, Redo, Undo};

mod assets;
mod config_manager;
mod config_panel_story;
mod generate_panel;
mod logs;
mod parse_panel;
mod progress_monitor;

pub use config_manager::{ConfigManager, ConfigSite};
pub use config_panel_story::ConfigPanelStory;
pub use generate_panel::GeneratePanelState;
use gpui::*;
pub use logs::{LogLevel, add_global_log, log_from_thread};
pub use parse_panel::ParsePanelState;
pub use progress_monitor::{
    ProgressMonitorPanel, TaskProgress, TaskStatus, clear_finished_tasks, get_task_progress,
    update_task_progress,
};
use story::AppState;

// actions!(main_menu, [Quit]);

// fn init(app_state: Arc<AppState>, cx: &mut AppContext) -> Result<()> {
//     story_workspace::init(app_state.clone(), cx);

//     cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

//     Ok(())
// }

pub fn run_gui() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        story::init(cx);
        cx.activate(true);

        let window = story::create_new_window("布置平台部署工具", ConfigPanelStory::view, cx);
    });
}
