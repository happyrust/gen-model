//! Buzhi platform UI theme tokens -- "Engineering Console" color system.
//!
//! Produced via `/impeccable` (product register) and materialized as ready-to-use
//! `gpui-component` `ThemeColor` tokens. Key decisions:
//!   - Layered neutrals (bg / panel / header / raised / hairline) build elevation.
//!   - A single accent ("engineering blue") is used ONLY for selection / primary / active.
//!   - Semantic colors (success / warning / danger / info) are used ONLY for state.
//!   - The 3D canvas is the deepest surface (the "stage"), pulled away from the chrome.
//!
//! Usage (call after `story::init(cx)` so the global Theme already exists):
//! ```ignore
//! story::init(cx);
//! crate::gui::theme_engineering_console::apply(cx, /* dark = */ true);
//! ```

use gpui::{App, px};
use gpui_component::{Colorize as _, Theme, ThemeColor, ThemeMode, hsl};

// ===================== Dark - Engineering Console =====================
//
//  role          hex          hsl(h,s,l)
//  bg (base)     #0E1116      (217, 22,  7)
//  panel/chrome  #161B22      (215, 21, 11)
//  header/tools  #1C232D      (215, 23, 14)
//  raised/input  #212A35      (214, 23, 17)
//  hairline      #2A3441      (214, 22, 21)
//  ink (text)    #E6EDF3      (208, 35, 93)
//  ink secondary #9BA8B7      (212, 16, 66)
//  ink muted     #6B7887      (212, 12, 47)
//  accent (blue) #4C9AFF      (214,100, 65)
//  success       #3FB950      (128, 49, 49)
//  warning       #D29922      ( 41, 72, 48)
//  danger        #F85149      (  3, 93, 63)
//  info          #58A6FF      (212,100, 67)

/// Dark "Engineering Console" token set.
pub fn engineering_console_dark() -> ThemeColor {
    let accent = hsl(214.0, 100.0, 65.0); // #4C9AFF
    let accent_hover = hsl(214.0, 100.0, 71.0);
    let accent_active = hsl(214.0, 90.0, 58.0);

    let ink = hsl(208.0, 35.0, 93.0); // primary text
    let ink_2 = hsl(212.0, 16.0, 66.0); // secondary text
    let ink_muted = hsl(212.0, 12.0, 47.0); // muted text

    let bg = hsl(217.0, 22.0, 7.0); // base
    let panel = hsl(215.0, 21.0, 11.0); // panel / chrome
    let header = hsl(215.0, 23.0, 14.0); // header / toolbar
    let raised = hsl(214.0, 23.0, 17.0); // raised / input / card
    let raised_hi = hsl(214.0, 23.0, 21.0);
    let hairline = hsl(214.0, 22.0, 21.0); // divider

    // Start from the official dark base so all ~80 fields are populated,
    // then override only what the palette needs.
    let mut c = ThemeColor::dark();

    // Base layer
    c.background = bg;
    c.foreground = ink;
    c.border = hairline;
    c.input = raised;
    c.ring = accent.opacity(0.55);
    c.caret = accent;
    c.selection = accent.opacity(0.30);
    c.muted = raised;
    c.muted_foreground = ink_muted;

    // Card / popover / accordion (right-side properties)
    c.card = panel;
    c.card_foreground = ink;
    c.popover = header;
    c.popover_foreground = ink;
    c.accordion = panel;
    c.accordion_hover = raised.opacity(0.7);
    c.accordion_active = raised;

    // Sidebar (left model tree / PBS tree)
    c.sidebar = panel;
    c.sidebar_foreground = ink_2;
    c.sidebar_border = hairline;
    c.sidebar_accent = raised;
    c.sidebar_accent_foreground = ink;
    c.sidebar_primary = accent;
    c.sidebar_primary_foreground = hsl(214.0, 60.0, 8.0);

    // Primary actions (= engineering blue)
    c.primary = accent;
    c.primary_hover = accent_hover;
    c.primary_active = accent_active;
    c.primary_foreground = hsl(214.0, 60.0, 8.0);
    c.progress_bar = accent;
    c.slider_bar = accent;
    c.slider_thumb = ink;

    // Secondary actions
    c.secondary = raised;
    c.secondary_hover = raised_hi;
    c.secondary_active = hsl(214.0, 23.0, 24.0);
    c.secondary_foreground = ink;

    // accent = list / menu-item hover background
    c.accent = raised;
    c.accent_foreground = ink;

    // Tabs (right properties / bottom logs)
    c.tab_bar = header;
    c.tab_bar_segmented = header;
    c.tab_active = panel;
    c.tab_foreground = ink_2;
    c.tab_active_foreground = ink;

    // Title bar
    c.title_bar = header;
    c.title_bar_border = hairline;

    // List (model tree)
    c.list = panel;
    c.list_head = header;
    c.list_even = bg;
    c.list_hover = raised.opacity(0.6);
    c.list_active = accent.opacity(0.16);
    c.list_active_border = accent;

    // Table (logs / property rows)
    c.table = panel;
    c.table_head = header;
    c.table_head_foreground = ink_2;
    c.table_even = bg;
    c.table_hover = raised.opacity(0.5);
    c.table_active = accent.opacity(0.16);
    c.table_active_border = accent;
    c.table_row_border = hairline.opacity(0.6);

    // Description list (general / component / UDA properties)
    c.description_list_label = header;
    c.description_list_label_foreground = ink_2;

    // Semantic colors -- state only
    c.success = hsl(128.0, 49.0, 49.0);
    c.success_hover = hsl(128.0, 49.0, 49.0).opacity(0.9);
    c.success_active = hsl(128.0, 49.0, 42.0);
    c.success_foreground = hsl(140.0, 60.0, 96.0);

    c.warning = hsl(41.0, 72.0, 48.0);
    c.warning_hover = hsl(41.0, 72.0, 48.0).opacity(0.9);
    c.warning_active = hsl(41.0, 72.0, 42.0);
    c.warning_foreground = hsl(45.0, 80.0, 10.0);

    c.danger = hsl(3.0, 93.0, 63.0);
    c.danger_hover = hsl(3.0, 93.0, 63.0).opacity(0.9);
    c.danger_active = hsl(3.0, 85.0, 55.0);
    c.danger_foreground = hsl(0.0, 0.0, 100.0);

    c.info = hsl(212.0, 100.0, 67.0);
    c.info_hover = hsl(212.0, 100.0, 67.0).opacity(0.9);
    c.info_active = hsl(212.0, 90.0, 60.0);
    c.info_foreground = hsl(0.0, 0.0, 100.0);

    // Links
    c.link = accent;
    c.link_hover = accent_hover;
    c.link_active = accent_active;

    // Scrollbar / skeleton / switch / drag
    c.scrollbar = bg.opacity(0.0);
    c.scrollbar_thumb = ink_muted.opacity(0.5);
    c.scrollbar_thumb_hover = ink_2.opacity(0.7);
    c.switch = hairline;
    c.skeleton = ink.opacity(0.08);
    c.drag_border = accent;
    c.drop_target = accent.opacity(0.15);
    c.tiles = bg;

    c
}

// ===================== Light - Precision Daylight =====================
//
//  role          hex          hsl(h,s,l)
//  bg (base)     #F4F6F9      (216, 29, 97)
//  panel         #FFFFFF      (  0,  0,100)
//  header/tools  #EEF2F7      (214, 36, 95)
//  hairline      #DCE3EC      (214, 30, 89)
//  ink (text)    #1B2430      (214, 28, 15)
//  ink secondary #566579      (214, 17, 41)
//  accent        #2563EB      (221, 83, 53)

/// Light "Precision Daylight" token set (same semantics, swapped palette).
pub fn engineering_console_light() -> ThemeColor {
    let accent = hsl(221.0, 83.0, 53.0); // #2563EB
    let accent_hover = hsl(221.0, 83.0, 47.0);
    let accent_active = hsl(221.0, 83.0, 42.0);

    let ink = hsl(214.0, 28.0, 15.0); // primary text
    let ink_2 = hsl(214.0, 17.0, 41.0); // secondary text
    let ink_muted = hsl(214.0, 14.0, 58.0); // muted text

    let bg = hsl(216.0, 29.0, 97.0); // base
    let panel = hsl(0.0, 0.0, 100.0); // panel
    let header = hsl(214.0, 36.0, 95.0); // header / toolbar
    let hairline = hsl(214.0, 30.0, 89.0); // divider

    let mut c = ThemeColor::light();

    c.background = bg;
    c.foreground = ink;
    c.border = hairline;
    c.input = hairline;
    c.ring = accent.opacity(0.45);
    c.caret = accent;
    c.selection = accent.opacity(0.20);
    c.muted = header;
    c.muted_foreground = ink_muted;

    c.card = panel;
    c.card_foreground = ink;
    c.popover = panel;
    c.popover_foreground = ink;
    c.accordion = panel;
    c.accordion_hover = header.opacity(0.7);
    c.accordion_active = header;

    c.sidebar = bg;
    c.sidebar_foreground = ink_2;
    c.sidebar_border = hairline;
    c.sidebar_accent = header;
    c.sidebar_accent_foreground = ink;
    c.sidebar_primary = accent;
    c.sidebar_primary_foreground = hsl(0.0, 0.0, 100.0);

    c.primary = accent;
    c.primary_hover = accent_hover;
    c.primary_active = accent_active;
    c.primary_foreground = hsl(0.0, 0.0, 100.0);
    c.progress_bar = accent;
    c.slider_bar = accent;
    c.slider_thumb = panel;

    c.secondary = header;
    c.secondary_hover = hsl(214.0, 36.0, 92.0);
    c.secondary_active = hsl(214.0, 34.0, 89.0);
    c.secondary_foreground = ink;

    c.accent = header;
    c.accent_foreground = ink;

    c.tab_bar = header;
    c.tab_bar_segmented = header;
    c.tab_active = panel;
    c.tab_foreground = ink_2;
    c.tab_active_foreground = ink;

    c.title_bar = panel;
    c.title_bar_border = hairline;

    c.list = panel;
    c.list_head = header;
    c.list_even = bg;
    c.list_hover = header.opacity(0.7);
    c.list_active = accent.opacity(0.12);
    c.list_active_border = accent;

    c.table = panel;
    c.table_head = header;
    c.table_head_foreground = ink_2;
    c.table_even = bg;
    c.table_hover = header.opacity(0.6);
    c.table_active = accent.opacity(0.12);
    c.table_active_border = accent;
    c.table_row_border = hairline.opacity(0.7);

    c.description_list_label = header;
    c.description_list_label_foreground = ink_2;

    c.success = hsl(142.0, 76.0, 36.0);
    c.success_hover = hsl(142.0, 76.0, 36.0).opacity(0.9);
    c.success_active = hsl(142.0, 76.0, 30.0);
    c.success_foreground = hsl(0.0, 0.0, 100.0);

    c.warning = hsl(32.0, 95.0, 44.0);
    c.warning_hover = hsl(32.0, 95.0, 44.0).opacity(0.9);
    c.warning_active = hsl(32.0, 95.0, 38.0);
    c.warning_foreground = hsl(0.0, 0.0, 100.0);

    c.danger = hsl(0.0, 72.0, 51.0);
    c.danger_hover = hsl(0.0, 72.0, 51.0).opacity(0.9);
    c.danger_active = hsl(0.0, 72.0, 44.0);
    c.danger_foreground = hsl(0.0, 0.0, 100.0);

    c.info = hsl(221.0, 83.0, 53.0);
    c.info_hover = hsl(221.0, 83.0, 53.0).opacity(0.9);
    c.info_active = hsl(221.0, 83.0, 46.0);
    c.info_foreground = hsl(0.0, 0.0, 100.0);

    c.link = accent;
    c.link_hover = accent_hover;
    c.link_active = accent_active;

    c.scrollbar = bg.opacity(0.0);
    c.scrollbar_thumb = ink_muted.opacity(0.5);
    c.scrollbar_thumb_hover = ink_2.opacity(0.6);
    c.switch = hsl(214.0, 20.0, 80.0);
    c.skeleton = ink.opacity(0.06);
    c.drag_border = accent;
    c.drop_target = accent.opacity(0.12);
    c.tiles = header;

    c
}

/// Apply the Engineering Console theme to the global [`Theme`]
/// (also sets radius / font size / CJK font).
///
/// Call after `story::init(cx)` / `gpui_component::init(cx)` (global Theme must exist).
pub fn apply(cx: &mut App, dark: bool) {
    let (colors, mode) = if dark {
        (engineering_console_dark(), ThemeMode::Dark)
    } else {
        (engineering_console_light(), ThemeMode::Light)
    };

    if !cx.has_global::<Theme>() {
        cx.set_global(Theme::from(colors));
    }

    let theme = Theme::global_mut(cx);
    theme.mode = mode;
    theme.colors = colors;
    theme.radius = px(6.0); // cards/inputs <= 8px, no over-rounding
    theme.shadow = true;
    theme.font_size = px(13.0); // fixed, dense-but-readable tool UI
    if cfg!(target_os = "windows") {
        theme.font_family = "Microsoft YaHei UI".into();
    }
}

/// Convenience: apply the dark Engineering Console theme.
pub fn apply_dark(cx: &mut App) {
    apply(cx, true);
}
