use eframe::egui;

use crate::ui::theme::{muted, section_border, section_surface, setting_separator_color};


pub(crate) fn settings_scroll_area<R>(
    ui: &mut egui::Ui,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .content_margin(egui::Margin::symmetric(16, 0))
                .show(ui, body)
                .inner
        })
        .inner
}

pub(crate) fn settings_section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(25.0).strong());
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(20, 8))
        .show(ui, body);
    ui.add_space(18.0);
}

pub(crate) fn setting_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let row_width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(row_width, 62.0), egui::Sense::hover());
    let control_width = row_width.min(360.0);
    let control_rect = egui::Rect::from_min_max(
        egui::pos2(row_rect.right() - control_width, row_rect.top()),
        row_rect.max,
    );
    let label_rect = egui::Rect::from_min_max(
        row_rect.min,
        egui::pos2(
            (control_rect.left() - 16.0).max(row_rect.left()),
            row_rect.bottom(),
        ),
    );
    let mut label_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(label_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    label_ui.set_clip_rect(label_rect.intersect(ui.clip_rect()));
    label_ui.add_space(8.0);
    label_ui.label(egui::RichText::new(title).size(16.0).strong());
    label_ui.label(egui::RichText::new(detail).size(14.0).color(muted()));

    let mut control_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(control_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control_ui.set_clip_rect(control_rect.intersect(ui.clip_rect()));
    control(&mut control_ui);
}

pub(crate) fn setting_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, setting_separator_color()),
    );
}


/// A row of tabs; the current one is underlined in the text colour.
pub(crate) fn tab_strip<T: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut T, tabs: &[(T, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        for (tab, title) in tabs {
            let selected = *current == *tab;
            let galley = ui.painter().layout_no_wrap(
                title.to_string(),
                egui::FontId::proportional(15.0),
                crate::ui::theme::text(),
            );
            let size = egui::vec2(galley.size().x, 30.0);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let colour = if selected || response.hovered() { crate::ui::theme::text() } else { muted() };
            ui.painter().text(
                egui::pos2(rect.left(), rect.center().y - 2.0),
                egui::Align2::LEFT_CENTER,
                *title,
                egui::FontId::proportional(15.0),
                colour,
            );
            if selected {
                let underline = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - 2.0),
                    egui::pos2(rect.right(), rect.bottom()),
                );
                ui.painter().rect_filled(underline, 1.0, crate::ui::theme::text());
            }
            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *current = *tab;
            }
        }
    });
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, section_border()));
}

/// A card: a framed block with an optional header line -- a title on the
/// left, whatever `header_right` draws on the right -- above the body.
pub(crate) fn card(
    ui: &mut egui::Ui,
    title: Option<&str>,
    header_right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(20, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(title) = title {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).size(17.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), header_right);
                });
                ui.add_space(4.0);
            }
            body(ui);
        });
    ui.add_space(14.0);
}
