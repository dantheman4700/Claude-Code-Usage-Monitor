use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::ui::tokens::CONTROL_HEIGHT;

pub(crate) fn icon_text(icon: LucideIcon, size: f32) -> egui::RichText {
    egui::RichText::new(icon.unicode().to_string())
        .family(egui::FontFamily::Name("lucide".into()))
        .size(size)
}

pub(crate) fn icon_only_button(icon: LucideIcon) -> egui::Button<'static> {
    egui::Button::new(icon_text(icon, 16.0)).min_size(egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::dropdown::Dropdown;

    #[test]
    fn standard_controls_share_one_height() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let mut heights = [0.0; 4];
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| {
                heights[0] = Dropdown::from_id_salt("probe")
                    .selected_text("Theme")
                    .show_ui(ui, |_| {})
                    .response
                    .rect
                    .height();
                heights[1] = ui.add(icon_only_button(LucideIcon::X)).rect.height();
                heights[2] = ui.button("Plain").rect.height();
                heights[3] = ui
                    .add(egui::Button::new(("Discard", icon_text(LucideIcon::X, 16.0))))
                    .rect
                    .height();
            });
        });
        assert_eq!(heights, [CONTROL_HEIGHT; 4]);
    }
}
