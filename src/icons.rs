//! Ícones vetoriais desenhados com o painter do egui (não dependem de fontes de emoji,
//! então aparecem iguais em qualquer sistema).

use eframe::egui::{self, pos2, vec2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};
use std::f32::consts::{PI, TAU};

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Record,
    Pause,
    Play,
    Stop,
    Monitor,
    Mic,
    Folder,
    Refresh,
    Check,
    Warning,
    Gear,
    Back,
    Close,
}

/// Desenha `icon` dentro do quadrado `r`.
pub fn paint(p: &Painter, r: Rect, icon: Icon, color: Color32) {
    let s = r.width().min(r.height());
    let r = Rect::from_center_size(r.center(), vec2(s, s));
    // Coordenadas normalizadas 0..1 dentro do quadrado.
    let at = |x: f32, y: f32| -> Pos2 { pos2(r.left() + x * s, r.top() + y * s) };
    let w = (s * 0.1).max(1.4);
    let stroke = Stroke::new(w, color);
    match icon {
        Icon::Record => {
            p.circle_filled(r.center(), s * 0.36, color);
        }
        Icon::Pause => {
            for x in [0.28, 0.58] {
                p.rect_filled(Rect::from_min_max(at(x, 0.18), at(x + 0.16, 0.82)), s * 0.03, color);
            }
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(0.28, 0.16), at(0.84, 0.5), at(0.28, 0.84)], color, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(at(0.22, 0.22), at(0.78, 0.78)), s * 0.06, color);
        }
        Icon::Monitor => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.14), at(0.92, 0.7)), s * 0.06, stroke, egui::StrokeKind::Inside);
            p.line_segment([at(0.5, 0.7), at(0.5, 0.86)], stroke);
            p.line_segment([at(0.3, 0.88), at(0.7, 0.88)], stroke);
        }
        Icon::Mic => {
            p.rect_filled(Rect::from_min_max(at(0.36, 0.06), at(0.64, 0.6)), s * 0.14, color);
            let c = at(0.5, 0.42);
            let arc: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = PI * i as f32 / 16.0;
                    pos2(c.x + a.cos() * s * 0.27, c.y + a.sin() * s * 0.27)
                })
                .collect();
            p.add(Shape::line(arc, stroke));
            p.line_segment([at(0.5, 0.69), at(0.5, 0.88)], stroke);
            p.line_segment([at(0.32, 0.9), at(0.68, 0.9)], stroke);
        }
        Icon::Folder => {
            let pts = vec![at(0.08, 0.22), at(0.38, 0.22), at(0.47, 0.32), at(0.92, 0.32), at(0.92, 0.82), at(0.08, 0.82)];
            p.add(Shape::closed_line(pts, stroke));
        }
        Icon::Refresh => {
            let c = r.center();
            let rad = s * 0.32;
            let (a0, a1) = (-PI * 0.35, TAU * 0.78 - PI * 0.35);
            let arc: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = a0 + (a1 - a0) * i as f32 / 24.0;
                    pos2(c.x + a.cos() * rad, c.y + a.sin() * rad)
                })
                .collect();
            let end = *arc.last().unwrap();
            p.add(Shape::line(arc, stroke));
            // ponta da seta, tangente ao fim do arco
            let t = vec2(-a1.sin(), a1.cos());
            let n = vec2(a1.cos(), a1.sin());
            let tip = end + t * s * 0.12;
            p.add(Shape::convex_polygon(
                vec![tip, end - t * s * 0.06 + n * s * 0.13, end - t * s * 0.06 - n * s * 0.13],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Check => {
            p.add(Shape::line(vec![at(0.16, 0.52), at(0.4, 0.76), at(0.84, 0.26)], Stroke::new(w * 1.2, color)));
        }
        Icon::Gear => {
            let c = r.center();
            for i in 0..8 {
                let a = TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * s * 0.3, c + d * s * 0.46], Stroke::new(w * 1.5, color));
            }
            p.circle_stroke(c, s * 0.3, stroke);
            p.circle_stroke(c, s * 0.11, stroke);
        }
        Icon::Back => {
            p.add(Shape::line(vec![at(0.62, 0.18), at(0.3, 0.5), at(0.62, 0.82)], Stroke::new(w * 1.2, color)));
        }
        Icon::Close => {
            let s2 = Stroke::new(w * 1.1, color);
            p.line_segment([at(0.25, 0.25), at(0.75, 0.75)], s2);
            p.line_segment([at(0.25, 0.75), at(0.75, 0.25)], s2);
        }
        Icon::Warning => {
            p.add(Shape::closed_line(vec![at(0.5, 0.1), at(0.94, 0.88), at(0.06, 0.88)], stroke));
            p.line_segment([at(0.5, 0.38), at(0.5, 0.62)], stroke);
            p.circle_filled(at(0.5, 0.75), w * 0.7, color);
        }
    }
}

/// Botão com ícone + texto centralizados.
pub fn button(ui: &mut Ui, icon: Icon, text: &str, size: Vec2, text_size: f32, fill: Option<Color32>, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&resp, selected);
        let enabled = ui.is_enabled();
        let bg = match fill {
            Some(f) if !enabled => f.gamma_multiply(0.4),
            Some(f) if resp.hovered() => f.gamma_multiply(1.15),
            Some(f) => f,
            None => visuals.weak_bg_fill,
        };
        let fg = if fill.is_some() { Color32::WHITE } else { visuals.text_color() };
        let fg = if enabled { fg } else { fg.gamma_multiply(0.5) };
        let p = ui.painter();
        p.rect(rect, visuals.corner_radius, bg, visuals.bg_stroke, egui::StrokeKind::Inside);
        let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(text_size), fg);
        let icon_size = text_size * 1.05;
        let gap = if text.is_empty() { 0.0 } else { text_size * 0.45 };
        let total = icon_size + gap + galley.size().x;
        let x0 = rect.center().x - total / 2.0;
        let icon_rect = Rect::from_center_size(pos2(x0 + icon_size / 2.0, rect.center().y), vec2(icon_size, icon_size));
        paint(p, icon_rect, icon, fg);
        p.galley(pos2(x0 + icon_size + gap, rect.center().y - galley.size().y / 2.0), galley, fg);
    }
    resp
}

/// Botão quadrado só com ícone.
pub fn small(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let h = ui.spacing().interact_size.y;
    button(ui, icon, "", vec2(h + 4.0, h), h * 0.62, None, false).on_hover_text(tooltip)
}
