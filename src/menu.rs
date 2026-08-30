//! The tray icon's right-click menu, built in Rust.
//!
//! Every item is a plain command: no document, no editor, no expression
//! language. What the user can do from the tray is short enough to read
//! here, and it mirrors the panel's Settings page: the same switches, the
//! same current values, so a change made in one shows in the other.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow, TrackPopupMenu,
    HMENU, MENU_ITEM_FLAGS, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
};

/// Appends one command to a menu.
type ItemFn<'a> = dyn Fn(HMENU, MENU_ITEM_FLAGS, u16, &str) + 'a;
/// Appends a submenu, filled by the callback.
type SubmenuFn<'a> = dyn Fn(HMENU, &str, &dyn Fn(HMENU)) + 'a;

use crate::app_settings::{
    Appearance, TrayIconMark, TrayIconMeasure, TrayIconMetric, TrayIconMode, TrayIconSettings,
    TrayIconStyle, TrayIconTone, POLL_15_MIN, POLL_1_HOUR, POLL_1_MIN, POLL_5_MIN,
};
use crate::localization::LanguageId;
use crate::native_interop::wide_str;
use crate::providers::{ProviderId, PROVIDER_DESCRIPTORS};
use crate::state::lock_state;
use crate::updater::InstallChannel;

pub const CMD_OPEN: u16 = 10;
pub const CMD_REFRESH: u16 = 11;
pub const CMD_STARTUP: u16 = 12;
pub const CMD_UPDATES: u16 = 13;
pub const CMD_EXIT: u16 = 14;
pub const CMD_SETTINGS: u16 = 15;
pub const CMD_FREQ_1MIN: u16 = 20;
pub const CMD_FREQ_5MIN: u16 = 21;
pub const CMD_FREQ_15MIN: u16 = 22;
pub const CMD_FREQ_1HOUR: u16 = 23;
// The first tray icon's options, 30..53; its provider, 70..77.
pub const CMD_TRAY_MODE_LOGO: u16 = 30;
pub const CMD_TRAY_MODE_TIGHTEST: u16 = 31;
pub const CMD_TRAY_MODE_PROVIDER: u16 = 32;
pub const CMD_TRAY_MODE_RUNDOWN: u16 = 33;
pub const CMD_TRAY_STYLE_NUMBER: u16 = 34;
pub const CMD_TRAY_STYLE_BAR: u16 = 35;
pub const CMD_TRAY_STYLE_RING: u16 = 36;
pub const CMD_TRAY_STYLE_COLUMN: u16 = 37;
pub const CMD_TRAY_TONE_AUTO: u16 = 38;
pub const CMD_TRAY_TONE_LIGHT: u16 = 39;
pub const CMD_TRAY_TONE_DARK: u16 = 40;
pub const CMD_APPEARANCE_AUTO: u16 = 41;
pub const CMD_APPEARANCE_DARK: u16 = 42;
pub const CMD_APPEARANCE_LIGHT: u16 = 43;
pub const CMD_TRAY_METRIC_TIGHTEST: u16 = 44;
pub const CMD_TRAY_METRIC_SESSION: u16 = 45;
pub const CMD_TRAY_METRIC_WEEKLY: u16 = 46;
pub const CMD_TRAY_METRIC_MONTHLY: u16 = 47;
pub const CMD_TRAY_MEASURE_USED: u16 = 48;
pub const CMD_TRAY_MEASURE_REMAINING: u16 = 49;
pub const CMD_TRAY_MARK_DIGITS: u16 = 50;
pub const CMD_TRAY_MARK_INITIALS: u16 = 51;
pub const CMD_TRAY_MARK_NONE: u16 = 52;
pub const CMD_TRAY_ALERT_COLOUR: u16 = 53;
pub const CMD_TRAY_ADD: u16 = 54;
pub const CMD_TRAY_REMOVE: u16 = 55;
pub const CMD_TRAY_METRIC_CREDITS: u16 = 56;
pub const CMD_TRAY_ICONS_PAGE: u16 = 57;
pub const CMD_TRAY_STYLE_LETTERS: u16 = 58;
const CMD_TRAY_PROVIDER_FIRST: u16 = 70;
/// A per-model cap of the icon's provider, by its place in the provider's
/// list; resolved to its label when applied.
const CMD_TRAY_SCOPED_FIRST: u16 = 80;
const CMD_TRAY_SCOPED_LAST: u16 = 99;
// Provider on/off toggles use each descriptor's own command id (60..).

/// What a menu command changes on the tray icon it came from, if it is one
/// of those. Plain data so the mapping can be tested without a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayIconChange {
    Mode(TrayIconMode),
    Provider(ProviderId),
    Metric(TrayIconMetric),
    /// The n-th per-model cap the icon's provider reports.
    ScopedWindow(usize),
    Measure(TrayIconMeasure),
    Style(TrayIconStyle),
    Mark(TrayIconMark),
    Tone(TrayIconTone),
    ToggleAlertColour,
    /// Another icon after this one, for a provider not yet on an icon.
    Add,
    /// This icon, unless it is the last.
    Remove,
}

impl TrayIconChange {
    pub fn for_command(id: u16) -> Option<Self> {
        Some(match id {
            CMD_TRAY_MODE_LOGO => Self::Mode(TrayIconMode::Logo),
            CMD_TRAY_MODE_TIGHTEST => Self::Mode(TrayIconMode::Tightest),
            CMD_TRAY_MODE_PROVIDER => Self::Mode(TrayIconMode::Provider),
            CMD_TRAY_MODE_RUNDOWN => Self::Mode(TrayIconMode::Rundown),
            CMD_TRAY_STYLE_NUMBER => Self::Style(TrayIconStyle::Number),
            CMD_TRAY_STYLE_BAR => Self::Style(TrayIconStyle::Bar),
            CMD_TRAY_STYLE_RING => Self::Style(TrayIconStyle::Ring),
            CMD_TRAY_STYLE_COLUMN => Self::Style(TrayIconStyle::Column),
            CMD_TRAY_STYLE_LETTERS => Self::Style(TrayIconStyle::Letters),
            CMD_TRAY_TONE_AUTO => Self::Tone(TrayIconTone::Auto),
            CMD_TRAY_TONE_LIGHT => Self::Tone(TrayIconTone::Light),
            CMD_TRAY_TONE_DARK => Self::Tone(TrayIconTone::Dark),
            CMD_TRAY_METRIC_TIGHTEST => Self::Metric(TrayIconMetric::Tightest),
            CMD_TRAY_METRIC_SESSION => Self::Metric(TrayIconMetric::Session),
            CMD_TRAY_METRIC_WEEKLY => Self::Metric(TrayIconMetric::Weekly),
            CMD_TRAY_METRIC_MONTHLY => Self::Metric(TrayIconMetric::Monthly),
            CMD_TRAY_MEASURE_USED => Self::Measure(TrayIconMeasure::Used),
            CMD_TRAY_MEASURE_REMAINING => Self::Measure(TrayIconMeasure::Remaining),
            CMD_TRAY_MARK_DIGITS => Self::Mark(TrayIconMark::Digits),
            CMD_TRAY_MARK_INITIALS => Self::Mark(TrayIconMark::Initials),
            CMD_TRAY_MARK_NONE => Self::Mark(TrayIconMark::None),
            CMD_TRAY_ALERT_COLOUR => Self::ToggleAlertColour,
            CMD_TRAY_ADD => Self::Add,
            CMD_TRAY_REMOVE => Self::Remove,
            CMD_TRAY_METRIC_CREDITS => Self::Metric(TrayIconMetric::Credits),
            id if (CMD_TRAY_SCOPED_FIRST..=CMD_TRAY_SCOPED_LAST).contains(&id) => Self::ScopedWindow((id - CMD_TRAY_SCOPED_FIRST) as usize),
            id => {
                let index = id.checked_sub(CMD_TRAY_PROVIDER_FIRST)? as usize;
                Self::Provider(*ProviderId::ALL.get(index)?)
            }
        })
    }

    /// Apply to icon `index` of the list; true when something changed. The
    /// list never goes empty.
    pub fn apply_to(
        self,
        icons: &mut Vec<TrayIconSettings>,
        index: usize,
        enabled: crate::providers::ProviderSet,
        data: Option<&crate::models::AppUsageData>,
    ) -> bool {
        match self {
            Self::ScopedWindow(nth) => {
                let Some(icon) = icons.get_mut(index) else {
                    return false;
                };
                let provider = icon.provider.as_deref().and_then(ProviderId::from_key).or_else(|| enabled.iter().next());
                let label = provider
                    .and_then(|provider| data?.get(provider))
                    .and_then(|usage| usage.scoped.get(nth))
                    .map(|scoped| scoped.label.clone());
                match label {
                    Some(label) => Self::Metric(TrayIconMetric::Scoped(label)).apply(icon),
                    None => false,
                }
            }
            Self::Add => {
                let after = index.min(icons.len());
                icons.insert(after.saturating_add(1).min(icons.len()), new_icon(icons, enabled));
                true
            }
            Self::Remove => {
                if icons.len() > 1 && index < icons.len() {
                    icons.remove(index);
                    true
                } else {
                    false
                }
            }
            change => icons.get_mut(index).is_some_and(|icon| change.apply(icon)),
        }
    }

    /// Apply to one icon's settings; true when something changed.
    pub fn apply(self, icon: &mut TrayIconSettings) -> bool {
        let before = icon.clone();
        match self {
            Self::Add | Self::Remove | Self::ScopedWindow(_) => return false,
            Self::Mode(mode) => icon.mode = mode,
            Self::Provider(provider) => {
                icon.provider = Some(provider.descriptor().key.to_string());
                icon.mode = TrayIconMode::Provider;
            }
            Self::Metric(metric) => icon.metric = metric,
            Self::Measure(measure) => icon.measure = measure,
            Self::Style(style) => icon.style = style,
            Self::Mark(mark) => icon.mark = mark,
            Self::Tone(tone) => icon.tone = tone,
            Self::ToggleAlertColour => icon.alert_colour = !icon.alert_colour,
        }
        *icon != before
    }
}

/// A new icon to add beside `icons`: one provider not yet on an icon
/// (the first enabled one when they all are), ringed, marked with its
/// initials so it can be told from the rest.
pub fn new_icon(icons: &[TrayIconSettings], enabled: crate::providers::ProviderSet) -> TrayIconSettings {
    let taken: Vec<&str> = icons.iter().filter_map(|icon| icon.provider.as_deref()).collect();
    let provider = enabled
        .iter()
        .map(|provider| provider.descriptor().key)
        .find(|key| !taken.contains(key))
        .or_else(|| enabled.iter().next().map(|provider| provider.descriptor().key));
    TrayIconSettings {
        mode: TrayIconMode::Provider,
        provider: provider.map(str::to_string),
        mark: TrayIconMark::Initials,
        ..Default::default()
    }
}

/// The appearance a menu command picks, if it is one of those.
pub fn appearance_for_command(id: u16) -> Option<Appearance> {
    match id {
        CMD_APPEARANCE_AUTO => Some(Appearance::Auto),
        CMD_APPEARANCE_DARK => Some(Appearance::Dark),
        CMD_APPEARANCE_LIGHT => Some(Appearance::Light),
        _ => None,
    }
}

/// The names the Settings page uses, so the two read alike.
pub fn mode_label(mode: TrayIconMode) -> &'static str {
    match mode {
        TrayIconMode::Logo => "The logo",
        TrayIconMode::Tightest => "Tightest limit across providers",
        TrayIconMode::Provider => "One provider",
        TrayIconMode::Rundown => "Every provider, as bars",
    }
}

pub fn measure_label(measure: TrayIconMeasure) -> &'static str {
    match measure {
        TrayIconMeasure::Used => "What is used",
        TrayIconMeasure::Remaining => "What is left",
    }
}

pub fn style_label(style: TrayIconStyle) -> &'static str {
    match style {
        TrayIconStyle::Number => "A number",
        TrayIconStyle::Bar => "A bar that fills",
        TrayIconStyle::Ring => "A ring that fills",
        TrayIconStyle::Column => "A column that fills",
        TrayIconStyle::Letters => "Letters that fill",
    }
}

pub fn mark_label(mark: TrayIconMark) -> &'static str {
    match mark {
        TrayIconMark::Digits => "The percent",
        TrayIconMark::Initials => "The label",
        TrayIconMark::None => "Nothing",
    }
}

/// Where a style puts its text, for the row's caption.
pub fn mark_place(style: TrayIconStyle) -> &'static str {
    match style {
        TrayIconStyle::Ring => "Inside the ring, once the icon is large enough to read",
        TrayIconStyle::Bar | TrayIconStyle::Column | TrayIconStyle::Number => "Above, once the icon is large enough to read",
        TrayIconStyle::Letters => "",
    }
}

pub fn tone_label(tone: TrayIconTone) -> &'static str {
    match tone {
        TrayIconTone::Auto => "Auto",
        TrayIconTone::Light => "Light (for a dark taskbar)",
        TrayIconTone::Dark => "Dark (for a light taskbar)",
    }
}

pub fn appearance_label(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Auto => "Auto",
        Appearance::Dark => "Dark",
        Appearance::Light => "Light",
    }
}

/// Show the menu at the cursor and return the chosen command, if any.
/// `icon` is the tray icon that was right-clicked; the icon submenu is its.
pub fn show(hwnd: HWND, icon: usize) -> Option<u16> {
    let (language, interval, providers, install_channel, icon, icon_count, appearance, data) = {
        let state = lock_state();
        let s = state.as_ref()?;
        (
            s.language,
            s.poll_interval_ms,
            s.providers,
            s.install_channel,
            s.tray_icons.get(icon).cloned().unwrap_or_default(),
            s.tray_icons.len(),
            s.appearance,
            s.data.clone(),
        )
    };
    // What the icon's provider reports, so its Window submenu offers the
    // provider's own limits.
    let usage = icon
        .provider
        .as_deref()
        .and_then(ProviderId::from_key)
        .or_else(|| providers.iter().next())
        .and_then(|provider| data.as_ref()?.get(provider).cloned());
    let startup = crate::tray::is_startup_enabled();

    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let item = |menu, flags, id: u16, label: &str| {
            let wide = wide_str(label);
            let _ = AppendMenuW(menu, flags, id as usize, PCWSTR::from_raw(wide.as_ptr()));
        };
        let checked = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
        let submenu = |parent: HMENU, label: &str, fill: &dyn Fn(HMENU)| {
            if let Ok(child) = CreatePopupMenu() {
                fill(child);
                let wide = wide_str(label);
                let _ = AppendMenuW(parent, MF_POPUP, child.0 as usize, PCWSTR::from_raw(wide.as_ptr()));
            }
        };
        let separator = |menu| {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        };

        item(menu, MF_STRING, CMD_OPEN, language.text("Open Headroom"));
        item(menu, MF_STRING, CMD_REFRESH, language.text("Refresh"));
        separator(menu);

        submenu(menu, language.text("Update frequency"), &|frequency| {
            for (id, value, label) in [
                (CMD_FREQ_1MIN, POLL_1_MIN, "Every minute"),
                (CMD_FREQ_5MIN, POLL_5_MIN, "Every 5 minutes"),
                (CMD_FREQ_15MIN, POLL_15_MIN, "Every 15 minutes"),
                (CMD_FREQ_1HOUR, POLL_1_HOUR, "Every hour"),
            ] {
                item(frequency, checked(interval == value), id, language.text(label));
            }
        });

        submenu(menu, language.text("Providers"), &|providers_menu| {
            for descriptor in PROVIDER_DESCRIPTORS {
                item(
                    providers_menu,
                    checked(providers.contains(descriptor.id)),
                    descriptor.native_menu_command_id,
                    language.text(descriptor.display_name),
                );
            }
        });

        let icon_title = if icon_count > 1 { language.text("This icon") } else { language.text("Tray icon") };
        submenu(menu, icon_title, &|tray| fill_tray_icon_menu(tray, &icon, icon_count, usage.as_ref(), language, &item, &submenu, &separator));

        submenu(menu, language.text("Appearance"), &|looks| {
            for (id, value) in [
                (CMD_APPEARANCE_AUTO, Appearance::Auto),
                (CMD_APPEARANCE_DARK, Appearance::Dark),
                (CMD_APPEARANCE_LIGHT, Appearance::Light),
            ] {
                item(looks, checked(appearance == value), id, language.text(appearance_label(value)));
            }
        });

        item(menu, checked(startup), CMD_STARTUP, language.text("Start with Windows"));
        if !matches!(install_channel, InstallChannel::Store) {
            item(menu, MF_STRING, CMD_UPDATES, language.text("Check for updates"));
        }
        item(menu, MF_STRING, CMD_SETTINGS, language.text("Settings…"));
        separator(menu);
        item(menu, MF_STRING, CMD_EXIT, language.text("Exit"));

        // The menu only dismisses on an outside click if this window is in
        // the foreground first; that is the documented tray-menu dance.
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let id = chosen.0 as u16;
        (id != 0).then_some(id)
    }
}

/// One icon's options, in the order the Settings page lists them. Options
/// that do not apply to the current mode are left out, as the page leaves
/// them out, so the menu never offers a switch that does nothing.
#[allow(clippy::too_many_arguments)]
fn fill_tray_icon_menu(
    tray: HMENU,
    icon: &TrayIconSettings,
    icon_count: usize,
    usage: Option<&crate::models::UsageData>,
    language: LanguageId,
    item: &ItemFn<'_>,
    submenu: &SubmenuFn<'_>,
    separator: &dyn Fn(HMENU),
) {
    let checked = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
    for (id, mode) in [
        (CMD_TRAY_MODE_LOGO, TrayIconMode::Logo),
        (CMD_TRAY_MODE_TIGHTEST, TrayIconMode::Tightest),
        (CMD_TRAY_MODE_PROVIDER, TrayIconMode::Provider),
        (CMD_TRAY_MODE_RUNDOWN, TrayIconMode::Rundown),
    ] {
        item(tray, checked(icon.mode == mode), id, language.text(mode_label(mode)));
    }
    let chosen_provider = icon.provider.as_deref().and_then(ProviderId::from_key);
    let shows_value = matches!(icon.mode, TrayIconMode::Tightest | TrayIconMode::Provider);
    if icon.mode != TrayIconMode::Logo {
        separator(tray);
    }
    if icon.mode == TrayIconMode::Provider {
        submenu(tray, language.text("Provider"), &|providers| {
            for (index, provider) in ProviderId::ALL.into_iter().enumerate() {
                item(
                    providers,
                    checked(chosen_provider == Some(provider)),
                    CMD_TRAY_PROVIDER_FIRST + index as u16,
                    language.text(provider.descriptor().display_name),
                );
            }
        });
    }
    if icon.mode != TrayIconMode::Logo {
        submenu(tray, language.text("Value"), &|window| {
            // One provider: the limits it reports, by their own names. The
            // fleet: the generic windows, applied to every provider.
            match usage.filter(|_| icon.mode == TrayIconMode::Provider) {
                Some(usage) => {
                    let mut scoped_index = 0u16;
                    for (metric, title) in crate::tray_paint::provider_windows(usage) {
                        let id = match &metric {
                            TrayIconMetric::Tightest => CMD_TRAY_METRIC_TIGHTEST,
                            TrayIconMetric::Session => CMD_TRAY_METRIC_SESSION,
                            TrayIconMetric::Weekly => CMD_TRAY_METRIC_WEEKLY,
                            TrayIconMetric::Monthly => CMD_TRAY_METRIC_MONTHLY,
                            TrayIconMetric::Credits => CMD_TRAY_METRIC_CREDITS,
                            TrayIconMetric::Scoped(_) => {
                                let id = CMD_TRAY_SCOPED_FIRST + scoped_index;
                                scoped_index += 1;
                                if id > CMD_TRAY_SCOPED_LAST {
                                    continue;
                                }
                                id
                            }
                        };
                        item(window, checked(icon.metric == metric), id, &title);
                    }
                }
                None => {
                    for (id, metric) in [
                        (CMD_TRAY_METRIC_TIGHTEST, TrayIconMetric::Tightest),
                        (CMD_TRAY_METRIC_SESSION, TrayIconMetric::Session),
                        (CMD_TRAY_METRIC_WEEKLY, TrayIconMetric::Weekly),
                        (CMD_TRAY_METRIC_MONTHLY, TrayIconMetric::Monthly),
                    ] {
                        item(window, checked(icon.metric == metric), id, &crate::tray_paint::metric_name(&metric, None));
                    }
                }
            }
        });
        submenu(tray, language.text("Shows"), &|shows| {
            for (id, measure) in [
                (CMD_TRAY_MEASURE_USED, TrayIconMeasure::Used),
                (CMD_TRAY_MEASURE_REMAINING, TrayIconMeasure::Remaining),
            ] {
                item(shows, checked(icon.measure == measure), id, language.text(measure_label(measure)));
            }
        });
    }
    if shows_value {
        submenu(tray, language.text("Style"), &|style_menu| {
            for (id, style) in [
                (CMD_TRAY_STYLE_RING, TrayIconStyle::Ring),
                (CMD_TRAY_STYLE_BAR, TrayIconStyle::Bar),
                (CMD_TRAY_STYLE_COLUMN, TrayIconStyle::Column),
                (CMD_TRAY_STYLE_NUMBER, TrayIconStyle::Number),
                (CMD_TRAY_STYLE_LETTERS, TrayIconStyle::Letters),
            ] {
                item(style_menu, checked(icon.style == style), id, language.text(style_label(style)));
            }
        });
        if icon.style != TrayIconStyle::Letters {
            submenu(tray, language.text("Text on the icon"), &|marks| {
                for (id, mark) in [
                    (CMD_TRAY_MARK_DIGITS, TrayIconMark::Digits),
                    (CMD_TRAY_MARK_INITIALS, TrayIconMark::Initials),
                    (CMD_TRAY_MARK_NONE, TrayIconMark::None),
                ] {
                    item(marks, checked(icon.mark == mark), id, language.text(mark_label(mark)));
                }
            });
        }
    } else if icon.mode == TrayIconMode::Rundown {
        submenu(tray, language.text("Layout"), &|layout| {
            item(layout, checked(icon.style != TrayIconStyle::Bar), CMD_TRAY_STYLE_RING, language.text("Columns"));
            item(layout, checked(icon.style == TrayIconStyle::Bar), CMD_TRAY_STYLE_BAR, language.text("Rows"));
        });
    }
    separator(tray);
    if icon.mode != TrayIconMode::Logo {
        item(tray, checked(icon.alert_colour), CMD_TRAY_ALERT_COLOUR, language.text("Colour at the warning line"));
    }
    submenu(tray, language.text("Tone"), &|tones| {
        for (id, tone) in [
            (CMD_TRAY_TONE_AUTO, TrayIconTone::Auto),
            (CMD_TRAY_TONE_LIGHT, TrayIconTone::Light),
            (CMD_TRAY_TONE_DARK, TrayIconTone::Dark),
        ] {
            item(tones, checked(icon.tone == tone), id, language.text(tone_label(tone)));
        }
    });
    separator(tray);
    item(tray, MF_STRING, CMD_TRAY_ADD, language.text("Add another icon"));
    if icon_count > 1 {
        item(tray, MF_STRING, CMD_TRAY_REMOVE, language.text("Remove this icon"));
    }
    item(tray, MF_STRING, CMD_TRAY_ICONS_PAGE, language.text("All icons…"));
}

/// The provider a menu command switches on or off, if it is one.
pub fn provider_for_command(id: u16) -> Option<ProviderId> {
    ProviderId::from_native_menu_command_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tray_command_maps_to_one_change_and_back() {
        let mut icon = TrayIconSettings::default();
        assert!(TrayIconChange::for_command(CMD_TRAY_STYLE_COLUMN).unwrap().apply(&mut icon));
        assert_eq!(icon.style, TrayIconStyle::Column);
        assert!(!TrayIconChange::for_command(CMD_TRAY_STYLE_COLUMN).unwrap().apply(&mut icon), "same again is no change");
        // Picking a provider also switches the icon to that provider.
        let grok = ProviderId::ALL.iter().position(|p| *p == ProviderId::Grok).unwrap() as u16;
        assert!(TrayIconChange::for_command(CMD_TRAY_PROVIDER_FIRST + grok).unwrap().apply(&mut icon));
        assert_eq!((icon.mode, icon.provider.as_deref()), (TrayIconMode::Provider, Some("grok")));
        assert!(TrayIconChange::for_command(CMD_TRAY_ALERT_COLOUR).unwrap().apply(&mut icon));
        assert!(icon.alert_colour);
        // No tray command collides with the fixed ones or the provider toggles.
        assert!(TrayIconChange::for_command(CMD_OPEN).is_none());
        assert!(TrayIconChange::for_command(CMD_FREQ_1HOUR).is_none());
        for descriptor in PROVIDER_DESCRIPTORS {
            assert!(TrayIconChange::for_command(descriptor.native_menu_command_id).is_none(), "{}", descriptor.key);
            assert!(appearance_for_command(descriptor.native_menu_command_id).is_none());
        }
        assert!(TrayIconChange::for_command(CMD_TRAY_PROVIDER_FIRST + ProviderId::ALL.len() as u16).is_none());
        assert_eq!(appearance_for_command(CMD_APPEARANCE_LIGHT), Some(Appearance::Light));
    }

    #[test]
    fn icons_are_added_beside_the_clicked_one_and_never_all_removed() {
        let enabled = crate::providers::ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
        let mut icons = vec![TrayIconSettings::default()];
        assert!(!TrayIconChange::Remove.apply_to(&mut icons, 0, enabled, None), "the last icon stays");
        assert!(TrayIconChange::Add.apply_to(&mut icons, 0, enabled, None));
        assert_eq!(icons.len(), 2);
        assert_eq!((icons[1].mode, icons[1].provider.as_deref(), icons[1].mark), (TrayIconMode::Provider, Some("claude"), TrayIconMark::Initials));
        assert!(TrayIconChange::Add.apply_to(&mut icons, 0, enabled, None));
        assert_eq!(icons[1].provider.as_deref(), Some("codex"), "a provider not yet on an icon, placed after the clicked one");
        assert_eq!(icons.len(), 3);
        assert!(TrayIconChange::Style(TrayIconStyle::Number).apply_to(&mut icons, 2, enabled, None));
        assert_eq!(icons[2].style, TrayIconStyle::Number);
        assert!(!TrayIconChange::Style(TrayIconStyle::Number).apply_to(&mut icons, 9, enabled, None), "no such icon");
        assert!(TrayIconChange::Remove.apply_to(&mut icons, 1, enabled, None));
        assert_eq!(icons.len(), 2);
    }

    #[test]
    fn a_per_model_cap_is_picked_by_its_place_and_kept_by_its_label() {
        let enabled = crate::providers::ProviderSet::from_enabled([ProviderId::Claude]);
        let mut data = crate::models::AppUsageData::default();
        let mut usage = crate::models::UsageData::default();
        usage.scoped.push(crate::models::ScopedLimit { label: "Fable".into(), window: Default::default(), section: Default::default() });
        data.insert(ProviderId::Claude, usage);
        let mut icons = vec![TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), ..Default::default() }];
        assert_eq!(TrayIconChange::for_command(CMD_TRAY_SCOPED_FIRST), Some(TrayIconChange::ScopedWindow(0)));
        assert!(TrayIconChange::ScopedWindow(0).apply_to(&mut icons, 0, enabled, Some(&data)));
        assert_eq!(icons[0].metric, TrayIconMetric::Scoped("Fable".into()));
        assert!(!TrayIconChange::ScopedWindow(3).apply_to(&mut icons, 0, enabled, Some(&data)), "no such cap");
        assert_eq!(TrayIconChange::for_command(CMD_TRAY_METRIC_CREDITS), Some(TrayIconChange::Metric(TrayIconMetric::Credits)));
    }
}
