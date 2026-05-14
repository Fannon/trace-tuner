use crate::{
    core::{midi_note_name, tuning_color, DetectionSnapshot, ResponseMode, TunerMode, TuningColor},
    SharedTunerState, TraceTunerParams, HISTORY_LEN,
};
use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::{
    create_egui_editor,
    egui::{
        self, Align, CentralPanel, Color32, FontFamily, FontId, Layout, Pos2, Rect, RichText,
        Sense, Stroke, Vec2,
    },
};
use std::sync::Arc;

const BG: Color32 = Color32::from_rgb(15, 17, 20);
const PANEL: Color32 = Color32::from_rgb(23, 26, 31);
const GRID: Color32 = Color32::from_rgb(52, 58, 66);
const TEXT: Color32 = Color32::from_rgb(226, 230, 235);
const MUTED: Color32 = Color32::from_rgb(116, 124, 134);
const BLUE: Color32 = Color32::from_rgb(82, 145, 255);
const TUNING_RANGE_CENTS: f32 = 35.0;
const IN_TUNE_CENTS: f32 = 10.0;
const HELD_CONFIDENCE_CUTOFF: f32 = 0.60;
const HISTORY_MIN_HEIGHT: f32 = 112.0;
const METER_HEIGHT: f32 = 22.0;
const CONTROL_HEIGHT: f32 = 24.0;

pub fn create_editor(
    params: Arc<TraceTunerParams>,
    shared_state: Arc<SharedTunerState>,
) -> Option<Box<dyn Editor>> {
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

            CentralPanel::default()
                .frame(egui::Frame::NONE.fill(BG))
                .show(egui_ctx, |ui| {
                    ui.set_min_size(Vec2::new(430.0, 255.0));
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 5.0);

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
        ui.add_space(8.0);
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
                .font(FontId::new(50.0, FontFamily::Proportional))
                .color(color),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
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

fn draw_history(ui: &mut egui::Ui, history: &[(f32, f32); HISTORY_LEN]) {
    let reserved_height = METER_HEIGHT + CONTROL_HEIGHT + ui.spacing().item_spacing.y * 2.0;
    let desired = Vec2::new(
        ui.available_width(),
        (ui.available_height() - reserved_height).max(HISTORY_MIN_HEIGHT),
    );
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, PANEL);

    let band = cents_to_y(rect, IN_TUNE_CENTS)..=cents_to_y(rect, -IN_TUNE_CENTS);
    let band_rect = Rect::from_min_max(
        Pos2::new(rect.left(), *band.start()),
        Pos2::new(rect.right(), *band.end()),
    );
    painter.rect_filled(
        band_rect,
        0.0,
        Color32::from_rgba_unmultiplied(51, 188, 116, 18),
    );

    for cents in [-35.0_f32, -20.0, -10.0, 0.0, 10.0, 20.0, 35.0] {
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
    let mut previous: Option<(Pos2, f32, f32)> = None;
    for (index, (cents, confidence)) in history.iter().copied().enumerate() {
        if cents.is_finite() {
            let point = Pos2::new(
                rect.left() + step * index as f32,
                cents_to_y(rect, cents.clamp(-TUNING_RANGE_CENTS, TUNING_RANGE_CENTS)),
            );
            if let Some((last, last_cents, last_confidence)) = previous {
                let segment_confidence = confidence.min(last_confidence);
                let stroke = Stroke::new(
                    2.5,
                    faded_line_color_for_cents((last_cents + cents) * 0.5, segment_confidence),
                );
                if segment_confidence < HELD_CONFIDENCE_CUTOFF {
                    draw_dotted_line(&painter, last, point, stroke);
                } else {
                    painter.line_segment([last, point], stroke);
                }
            }
            previous = Some((point, cents, confidence));
        } else {
            previous = None;
        }
    }
}

fn draw_meter(ui: &mut egui::Ui, snapshot: DetectionSnapshot) {
    let desired = Vec2::new(ui.available_width(), METER_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, PANEL);
    let band_left = cents_to_x(rect, -IN_TUNE_CENTS);
    let band_right = cents_to_x(rect, IN_TUNE_CENTS);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(band_left, rect.top()),
            Pos2::new(band_right, rect.bottom()),
        ),
        0.0,
        Color32::from_rgba_unmultiplied(51, 188, 116, 24),
    );

    for cents in [-35.0_f32, -20.0, -10.0, 0.0, 10.0, 20.0, 35.0] {
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
        let x = cents_to_x(
            rect,
            snapshot
                .cents
                .clamp(-TUNING_RANGE_CENTS, TUNING_RANGE_CENTS),
        );
        let stroke = Stroke::new(
            4.0,
            faded_line_color_for_cents(snapshot.cents, snapshot.confidence),
        );
        let top = Pos2::new(x, rect.top());
        let bottom = Pos2::new(x, rect.bottom());
        if snapshot.confidence < HELD_CONFIDENCE_CUTOFF {
            draw_dotted_line(&painter, top, bottom, stroke);
        } else {
            painter.line_segment([top, bottom], stroke);
        }
    }
}

fn draw_controls(ui: &mut egui::Ui, params: &TraceTunerParams, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
        toggle_button(
            ui,
            setter,
            &params.mode,
            ToggleSpec {
                first: TunerMode::Chromatic,
                second: TunerMode::Guitar,
                first_label: "Chromatic",
                second_label: "Guitar",
                width: 92.0,
            },
        );
        toggle_button(
            ui,
            setter,
            &params.response,
            ToggleSpec {
                first: ResponseMode::Stable,
                second: ResponseMode::Fast,
                first_label: "Stable",
                second_label: "Fast",
                width: 82.0,
            },
        );

        ui.add_space((ui.available_width() - 116.0).max(0.0));
        step_button(ui, setter, params, -0.1, "-");
        ui.label(
            RichText::new(format!("{:.1} Hz", params.reference_pitch.value()))
                .font(FontId::new(14.0, FontFamily::Proportional))
                .color(TEXT),
        );
        step_button(ui, setter, params, 0.1, "+");
    });
}

struct ToggleSpec<T> {
    first: T,
    second: T,
    first_label: &'static str,
    second_label: &'static str,
    width: f32,
}

fn toggle_button<T>(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &nih_plug::prelude::EnumParam<T>,
    spec: ToggleSpec<T>,
) where
    T: nih_plug::prelude::Enum + Copy + PartialEq + 'static,
{
    let value = param.value();
    let (next, label) = if value == spec.first {
        (spec.second, spec.first_label)
    } else {
        (spec.first, spec.second_label)
    };

    if ui
        .add_sized(
            [spec.width, CONTROL_HEIGHT],
            egui::Button::new(label).fill(Color32::from_rgb(42, 46, 52)),
        )
        .clicked()
    {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, next);
        setter.end_set_parameter(param);
    }
}

fn step_button(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &TraceTunerParams,
    amount: f32,
    label: &str,
) {
    if ui
        .add_sized([28.0, CONTROL_HEIGHT], egui::Button::new(label))
        .clicked()
    {
        let value = (params.reference_pitch.value() + amount).clamp(430.0, 450.0);
        setter.begin_set_parameter(&params.reference_pitch);
        setter.set_parameter(&params.reference_pitch, value);
        setter.end_set_parameter(&params.reference_pitch);
    }
}

fn cents_to_y(rect: Rect, cents: f32) -> f32 {
    rect.center().y - (cents / TUNING_RANGE_CENTS) * rect.height() * 0.5
}

fn cents_to_x(rect: Rect, cents: f32) -> f32 {
    rect.center().x + (cents / TUNING_RANGE_CENTS) * rect.width() * 0.5
}

fn color_for_cents(cents: f32) -> Color32 {
    match tuning_color(cents) {
        TuningColor::Green => Color32::from_rgb(51, 188, 116),
        TuningColor::Yellow => Color32::from_rgb(232, 196, 74),
        TuningColor::OrangeRed => Color32::from_rgb(235, 91, 78),
    }
}

fn line_color_for_cents(cents: f32) -> Color32 {
    match tuning_color(cents) {
        TuningColor::Green => Color32::from_rgb(83, 232, 145),
        TuningColor::Yellow => Color32::from_rgb(242, 210, 92),
        TuningColor::OrangeRed => Color32::from_rgb(245, 111, 92),
    }
}

fn faded_line_color_for_cents(cents: f32, confidence: f32) -> Color32 {
    let color = line_color_for_cents(cents);
    let alpha = if confidence >= HELD_CONFIDENCE_CUTOFF {
        255
    } else {
        (70.0 + confidence.max(0.0) / HELD_CONFIDENCE_CUTOFF * 100.0).round() as u8
    };
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn draw_dotted_line(painter: &egui::Painter, start: Pos2, end: Pos2, stroke: Stroke) {
    let delta = end - start;
    let length = delta.length();
    if length <= 0.0 {
        return;
    }

    let direction = delta / length;
    let mut distance = 0.0;
    while distance < length {
        let segment_end = (distance + 3.0).min(length);
        painter.line_segment(
            [
                start + direction * distance,
                start + direction * segment_end,
            ],
            stroke,
        );
        distance += 7.0;
    }
}
