//! The panel: a separate process the tray launches, showing the dashboard,
//! the tray icons and settings.

mod app;
mod dashboard;
mod settings;

pub use app::handle_cli_mode;

// The prelude the pages share.
pub(crate) use app::PanelApp;
pub(crate) use eframe::egui;
pub(crate) use crate::app_settings::{POLL_15_MIN, POLL_1_HOUR, POLL_1_MIN, POLL_5_MIN};
pub(crate) use crate::localization::LanguageId;
pub(crate) use crate::providers::PROVIDER_DESCRIPTORS;
pub(crate) use crate::ui::components::dropdown::{dropdown_selectable_value, Dropdown};
pub(crate) use crate::ui::components::layout::{
    setting_row, setting_separator, settings_scroll_area, settings_section as section,
};
pub(crate) use crate::ui::components::number_field::NumberField;
pub(crate) use crate::ui::components::toggle::Toggle;
pub(crate) use crate::ui::theme::configure_style;
pub(crate) use crate::ui::tokens::{TYPE_HERO, TYPE_LG, TYPE_MD, TYPE_SM, TYPE_XL, TYPE_XS};
