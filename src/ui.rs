use crate::{
    core::{midi_note_name, tuning_color, DetectionSnapshot, TuningColor},
    SharedTunerState, TraceTunerParams, HISTORY_LEN,
};
use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::{
    create_egui_editor,
    egui::{
        self, Align, Color32, FontFamily, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
    },
    resizable_window::ResizableWindow,
    widgets,
};
use std::sync::Arc;

const BG: Color32 = Color32::from_rgb(15, 17, 20);
const PANEL: Color32 = Color32::from_rgb(23, 26, 31);
const GRID: Color32 = Color32::from_rgb(52, 58, 66);
const TEXT: Color32 = Color32::from_rgb(226, 230, 235);
const MUTED: Color32 = Color32::from_rgb(116, 124, 134);
const BLUE: Color32 = Color32::from_rgb(82, 145, 255);

pub fn create_editor(
    params: Arc<TraceTunerParams>,
    shared_state: Arc<SharedTunerState>,
) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();

    create_egui_editor(
        params.editor_state.clone(),
        (),
        |ctx, _| {
            let mut visuals = egui::Visuals::dark();
            visuals.window_fill = BG;
            visuals.panel_fill = BG;
            visuals.widgets.active.bg_fill = BLUE;
            visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 92, 178);
            visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 38, 45);
            ctx.set_visuals(visuals);
        },
        move |egui_ctx, setter, _state| {
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));

            ResizableWindow::new("trace-tuner")
                .min_size(Vec2::new(360.0, 270.0))
                .show(egui_ctx, egui_state.as_ref(), |ui| {
                    ui.set_min_size(Vec2::new(360.0, 270.0));
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                    let snapshot = shared_state.snapshot();
                    let history = shared_state.history();

                    draw_header(ui, snapshot);
                    draw_history(ui, &history);
                    draw_meter(ui, snapshot);
                    draw_controls(ui, &params, setter);
                });
        },
    )
}

fn draw_header(ui: &mut egui::Ui, snapshot: DetectionSnapshot) {
    ui.horizontal(|ui| {
        let color = if snapshot.active {
            color_for_cents(snapshot.cents)
        } else {
            MUTED
        };
        let note = if snapshot.active {
            midi_note_name(snapshot.midi_note)
        } else {
            "--".to_owned()
        };

        ui.label(
            RichText::new(note)
                .font(FontId::new(58.0, FontFamily::Proportional))
                .color(color),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cents = if snapshot.active {
                format!("{:+.1} cents", snapshot.cents)
            } else {
                "--.- cents".to_owned()
            };
            ui.label(
                RichText::new(cents)
                    .font(FontId::new(17.0, FontFamily::Proportional))
                    .color(color),
            );

            let frequency = if snapshot.active {
                format!("{:.1} Hz", snapshot.frequency_hz)
            } else {
                "---.- Hz".to_owned()
            };
            ui.label(
                RichText::new(frequency)
                    .font(FontId::new(17.0, FontFamily::Proportional))
                    .color(TEXT),
            );
        });
    });
}

fn draw_history(ui: &mut egui::Ui, history: &[f32; HISTORY_LEN]) {
    let desired = Vec2::new(ui.available_width(), 128.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, PANEL);

    let band = cents_to_y(rect, 5.0)..=cents_to_y(rect, -5.0);
    let band_rect = Rect::from_min_max(
        Pos2::new(rect.left(), *band.start()),
        Pos2::new(rect.right(), *band.end()),
    );
    painter.rect_filled(
        band_rect,
        0.0,
        Color32::from_rgba_unmultiplied(51, 188, 116, 28),
    );

    for cents in [-50.0_f32, -25.0, 0.0, 25.0, 50.0] {
        let y = cents_to_y(rect, cents);
        let stroke = if cents == 0.0 {
            Stroke::new(1.5, Color32::from_rgb(92, 102, 113))
        } else {
            Stroke::new(1.0, GRID)
        };
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
    }

    let step = rect.width() / (HISTORY_LEN.saturating_sub(1)) as f32;
    let mut previous: Option<(Pos2, f32)> = None;
    for (index, cents) in history.iter().copied().enumerate() {
        if cents.is_finite() {
            let point = Pos2::new(
                rect.left() + step * index as f32,
                cents_to_y(rect, cents.clamp(-50.0, 50.0)),
            );
            if let Some((last, last_cents)) = previous {
                painter.line_segment(
                    [last, point],
                    Stroke::new(2.0, color_for_cents((last_cents + cents) * 0.5)),
                );
            }
            previous = Some((point, cents));
        } else {
            previous = None;
        }
    }
}

fn draw_meter(ui: &mut egui::Ui, snapshot: DetectionSnapshot) {
    let desired = Vec2::new(ui.available_width(), 22.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, PANEL);
    let band_left = cents_to_x(rect, -5.0);
    let band_right = cents_to_x(rect, 5.0);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(band_left, rect.top()),
            Pos2::new(band_right, rect.bottom()),
        ),
        0.0,
        Color32::from_rgba_unmultiplied(51, 188, 116, 36),
    );

    for cents in [-50.0_f32, -25.0, 0.0, 25.0, 50.0] {
        let x = cents_to_x(rect, cents);
        let stroke = if cents == 0.0 {
            Stroke::new(1.5, Color32::from_rgb(128, 138, 148))
        } else {
            Stroke::new(1.0, GRID)
        };
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
    }

    if snapshot.active {
        let x = cents_to_x(rect, snapshot.cents.clamp(-50.0, 50.0));
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(4.0, color_for_cents(snapshot.cents)),
        );
    }
}

fn draw_controls(ui: &mut egui::Ui, params: &TraceTunerParams, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mode").color(MUTED));
        ui.add(widgets::ParamSlider::for_param(&params.mode, setter).with_width(96.0));
        ui.label(RichText::new("Response").color(MUTED));
        ui.add(widgets::ParamSlider::for_param(&params.response, setter).with_width(104.0));
        ui.label(RichText::new("A4").color(MUTED));
        ui.add(widgets::ParamSlider::for_param(&params.reference_pitch, setter).with_width(88.0));
    });
}

fn cents_to_y(rect: Rect, cents: f32) -> f32 {
    rect.center().y - (cents / 50.0) * rect.height() * 0.5
}

fn cents_to_x(rect: Rect, cents: f32) -> f32 {
    rect.center().x + (cents / 50.0) * rect.width() * 0.5
}

fn color_for_cents(cents: f32) -> Color32 {
    match tuning_color(cents) {
        TuningColor::Green => Color32::from_rgb(51, 188, 116),
        TuningColor::Yellow => Color32::from_rgb(232, 196, 74),
        TuningColor::OrangeRed => Color32::from_rgb(235, 91, 78),
    }
}
