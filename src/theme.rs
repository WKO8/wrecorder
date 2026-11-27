//! Visual "glass": fontes, cores, fundo em gradiente e componentes próprios
//! (controle segmentado, chave liga/desliga, botões circulares, barra de título).

use crate::config::ThemeName;
use crate::icons::{self, Icon};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Rect, Response, Sense, Shadow, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2,
};
use std::sync::{Arc, RwLock};

// Cores semânticas (iguais em todos os temas).
pub const RED: Color32 = Color32::from_rgb(255, 69, 58);
pub const AMBER: Color32 = Color32::from_rgb(255, 170, 40);
pub const GREEN: Color32 = Color32::from_rgb(48, 199, 88);
pub const RADIUS: u8 = 14;

/// Mancha de luz do fundo: (x, y, raio, cor, intensidade), x/y relativos à janela.
type Blob = (f32, f32, f32, [f32; 3], f32);

/// Paleta de um tema.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Cor de destaque (seleções, chaves, botão Salvar).
    pub accent: Color32,
    /// Texto sobre a cor de destaque.
    pub on_accent: Color32,
    /// Botão de gravar.
    pub record: Color32,
    pub base_top: [f32; 3],
    pub base_bottom: [f32; 3],
    pub blobs: &'static [Blob],
    pub text: [Color32; 3],
    /// Fundo dos menus suspensos.
    pub popup: Color32,
    /// Cartões de vidro.
    pub card: Color32,
    /// "Pílula" selecionada do controle segmentado.
    pub thumb: Color32,
}

pub fn palette(name: ThemeName) -> Palette {
    let dark_text = [Color32::from_white_alpha(240), Color32::from_white_alpha(165), Color32::from_white_alpha(105)];
    let dark = |accent: Color32, on_accent: Color32, record: Color32, top: [f32; 3], bottom: [f32; 3], blobs: &'static [Blob]| Palette {
        dark: true,
        accent,
        on_accent,
        record,
        base_top: top,
        base_bottom: bottom,
        blobs,
        text: dark_text,
        popup: Color32::from_rgba_unmultiplied(top[0] as u8 + 18, top[1] as u8 + 18, top[2] as u8 + 22, 250),
        card: Color32::from_white_alpha(13),
        thumb: Color32::from_white_alpha(40),
    };
    match name {
        ThemeName::Red => dark(
            Color32::from_rgb(255, 69, 58),
            Color32::WHITE,
            Color32::from_rgb(255, 69, 58),
            [16.0, 10.0, 14.0],
            [24.0, 12.0, 18.0],
            &[
                (0.10, 0.08, 0.60, [214.0, 40.0, 60.0], 0.50),
                (1.00, 0.45, 0.55, [160.0, 30.0, 110.0], 0.38),
                (0.05, 0.98, 0.60, [235.0, 95.0, 50.0], 0.32),
                (0.85, 1.05, 0.40, [120.0, 30.0, 90.0], 0.30),
            ],
        ),
        ThemeName::Green => Palette {
            text: [Color32::from_rgb(214, 255, 226), Color32::from_rgba_unmultiplied(150, 230, 175, 190), Color32::from_rgba_unmultiplied(110, 200, 140, 140)],
            ..dark(
                Color32::from_rgb(0, 230, 118),
                Color32::from_rgb(0, 26, 10),
                Color32::from_rgb(0, 230, 118),
                [5.0, 11.0, 7.0],
                [6.0, 16.0, 10.0],
                &[
                    (0.10, 0.08, 0.60, [0.0, 170.0, 80.0], 0.42),
                    (1.00, 0.45, 0.55, [0.0, 120.0, 100.0], 0.32),
                    (0.05, 0.98, 0.60, [70.0, 160.0, 30.0], 0.30),
                    (0.85, 1.05, 0.40, [0.0, 90.0, 60.0], 0.30),
                ],
            )
        },
        ThemeName::Yellow => dark(
            Color32::from_rgb(255, 204, 0),
            Color32::from_rgb(30, 22, 0),
            Color32::from_rgb(255, 190, 0),
            [16.0, 13.0, 8.0],
            [22.0, 16.0, 9.0],
            &[
                (0.10, 0.08, 0.60, [230.0, 170.0, 20.0], 0.45),
                (1.00, 0.45, 0.55, [240.0, 110.0, 30.0], 0.32),
                (0.05, 0.98, 0.60, [200.0, 140.0, 40.0], 0.32),
                (0.85, 1.05, 0.40, [180.0, 80.0, 20.0], 0.28),
            ],
        ),
        ThemeName::Blue => dark(
            Color32::from_rgb(10, 132, 255),
            Color32::WHITE,
            Color32::from_rgb(255, 69, 58),
            [11.0, 12.0, 22.0],
            [17.0, 15.0, 34.0],
            &[
                (0.10, 0.08, 0.60, [94.0, 92.0, 230.0], 0.55),
                (1.00, 0.45, 0.55, [40.0, 110.0, 230.0], 0.38),
                (0.05, 0.98, 0.60, [24.0, 170.0, 180.0], 0.40),
                (0.85, 1.05, 0.40, [120.0, 70.0, 220.0], 0.30),
            ],
        ),
        ThemeName::Black => Palette {
            popup: Color32::from_rgba_unmultiplied(26, 26, 28, 252),
            card: Color32::from_white_alpha(10),
            ..dark(
                Color32::from_rgb(235, 235, 240),
                Color32::from_rgb(10, 10, 12),
                Color32::from_rgb(255, 69, 58),
                [9.0, 9.0, 10.0],
                [13.0, 13.0, 15.0],
                &[],
            )
        },
        ThemeName::White => Palette {
            dark: false,
            accent: Color32::from_rgb(255, 59, 48),
            on_accent: Color32::WHITE,
            record: Color32::from_rgb(255, 59, 48),
            base_top: [246.0, 246.0, 250.0],
            base_bottom: [232.0, 232.0, 240.0],
            blobs: &[
                (0.10, 0.08, 0.60, [255.0, 205.0, 210.0], 0.45),
                (1.00, 0.45, 0.55, [210.0, 215.0, 255.0], 0.40),
                (0.05, 0.98, 0.60, [200.0, 240.0, 235.0], 0.40),
            ],
            text: [
                Color32::from_rgb(28, 28, 32),
                Color32::from_rgba_unmultiplied(40, 40, 50, 170),
                Color32::from_rgba_unmultiplied(40, 40, 50, 115),
            ],
            popup: Color32::from_rgba_unmultiplied(252, 252, 254, 252),
            card: Color32::from_white_alpha(150),
            thumb: Color32::WHITE,
        },
    }
}

static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);

/// Paleta ativa.
pub fn pal() -> Palette {
    CURRENT.read().ok().and_then(|p| *p).unwrap_or_else(|| palette(ThemeName::Red))
}

pub fn accent() -> Color32 {
    pal().accent
}

/// Camada translúcida: branca nos temas escuros, preta no tema claro.
pub fn white(a: u8) -> Color32 {
    if pal().dark {
        Color32::from_white_alpha(a)
    } else {
        Color32::from_black_alpha((a as f32 * 0.6) as u8)
    }
}

/// Cor de texto principal / secundária / terciária.
pub fn text(level: u8) -> Color32 {
    pal().text[(level as usize).min(2)]
}

// ----------------------------------------------------------------- fontes

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn thin() -> FontFamily {
    FontFamily::Name("thin".into())
}

/// Usa a fonte do sistema (Ubuntu Sans / Segoe UI Variable / Inter...) com pesos variados.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // (arquivo variável, [arquivos estáticos regular, semibold, fino])
    let variable: &[&str] = if cfg!(windows) {
        &[r"C:\Windows\Fonts\SegUIVar.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/ubuntu/UbuntuSans[wdth,wght].ttf",
            "/usr/share/fonts/opentype/inter/InterVariable.ttf",
            "/usr/share/fonts/truetype/inter/InterVariable.ttf",
        ]
    };
    let statics: &[[&str; 3]] = if cfg!(windows) {
        &[[r"C:\Windows\Fonts\segoeui.ttf", r"C:\Windows\Fonts\seguisb.ttf", r"C:\Windows\Fonts\segoeuil.ttf"]]
    } else {
        &[
            [
                "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
                "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
                "/usr/share/fonts/truetype/ubuntu/Ubuntu-L.ttf",
            ],
            [
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Light.ttf",
            ],
        ]
    };

    let mut faces: Option<[FontData; 3]> = None;
    if let Some(bytes) = variable.iter().find_map(|p| std::fs::read(p).ok()) {
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let weight = |w: f32| {
            FontData::from_static(bytes).tweak(egui::FontTweak {
                coords: egui::epaint::text::VariationCoords::new([("wght", w)]),
                ..Default::default()
            })
        };
        faces = Some([weight(420.0), weight(600.0), weight(250.0)]);
    } else if let Some(set) = statics.iter().find(|s| std::path::Path::new(s[0]).exists()) {
        let load = |p: &str, fallback: &str| {
            let b = std::fs::read(p).or_else(|_| std::fs::read(fallback)).unwrap_or_default();
            FontData::from_owned(b)
        };
        faces = Some([load(set[0], set[0]), load(set[1], set[0]), load(set[2], set[0])]);
    }

    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some([regular, bold, light]) = faces {
        for (name, data) in [("ui-regular", regular), ("ui-semibold", bold), ("ui-thin", light)] {
            fonts.font_data.insert(name.into(), Arc::new(data));
        }
        let with = |first: &str| std::iter::once(first.to_string()).chain(fallback.iter().cloned()).collect::<Vec<_>>();
        fonts.families.insert(FontFamily::Proportional, with("ui-regular"));
        fonts.families.insert(semibold(), with("ui-semibold"));
        fonts.families.insert(thin(), with("ui-thin"));
    } else {
        fonts.families.insert(semibold(), fallback.clone());
        fonts.families.insert(thin(), fallback);
    }
    ctx.set_fonts(fonts);
}

// ----------------------------------------------------------------- estilo

/// Ativa um tema (cores + estilo dos widgets).
pub fn apply_style(ctx: &egui::Context, name: ThemeName) {
    use egui::{TextStyle, Visuals};
    let p = palette(name);
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(p);
    }
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = p.popup;
    v.window_stroke = Stroke::new(1.0, white(28));
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(12);
    v.popup_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    v.window_shadow = v.popup_shadow;
    v.extreme_bg_color = white(12);
    v.faint_bg_color = white(6);
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);

    let r = CornerRadius::same(9);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, text(0));
    w.noninteractive.bg_stroke = Stroke::new(1.0, white(18));
    w.noninteractive.corner_radius = r;
    for (wv, bg, stroke) in [
        (&mut w.inactive, 14, 14),
        (&mut w.hovered, 24, 34),
        (&mut w.active, 34, 44),
        (&mut w.open, 24, 34),
    ] {
        wv.bg_fill = white(bg);
        wv.weak_bg_fill = white(bg);
        wv.bg_stroke = Stroke::new(1.0, white(stroke));
        wv.corner_radius = r;
        wv.fg_stroke = Stroke::new(1.5, text(0));
        wv.expansion = 0.0;
    }
    ctx.set_visuals_of(if p.dark { egui::Theme::Dark } else { egui::Theme::Light }, v);
    ctx.set_theme(if p.dark { egui::Theme::Dark } else { egui::Theme::Light });

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.interact_size = vec2(40.0, 30.0);
        s.spacing.combo_height = 320.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(TextStyle::Heading, FontId::new(18.0, semibold()));
    });
}

// ----------------------------------------------------------------- fundo

/// Gradiente escuro com "manchas" de cor desfocadas (o efeito vidro aparece por cima delas).
pub fn background_texture(ctx: &egui::Context) -> TextureHandle {
    let (w, h) = (200usize, 330usize);
    let p = pal();
    let aspect = h as f32 / w as f32;
    let mut seed: u32 = 0x9e3779b9;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32 * aspect);
            let t = y as f32 / h as f32;
            let mut c = [0, 1, 2].map(|i| p.base_top[i] + (p.base_bottom[i] - p.base_top[i]) * t);
            for &(bx, by, r, col, k) in p.blobs {
                let by = by * aspect;
                let d2 = (fx - bx).powi(2) + (fy - by).powi(2);
                let a = k * (-d2 / (r * r * 0.35)).exp();
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - a) + col[i] * a;
                }
            }
            // ruído sutil contra "faixas" no gradiente
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((seed >> 24) as f32 / 255.0 - 0.5) * 3.0;
            px.push(Color32::from_rgb(
                (c[0] + n).clamp(0.0, 255.0) as u8,
                (c[1] + n).clamp(0.0, 255.0) as u8,
                (c[2] + n).clamp(0.0, 255.0) as u8,
            ));
        }
    }
    let img = ColorImage::new([w, h], px);
    ctx.load_texture("wrecorder-bg", img, TextureOptions::LINEAR)
}

/// Pinta o fundo arredondado da janela.
pub fn paint_window(ui: &Ui, rect: Rect, tex: &TextureHandle) {
    let p = ui.painter();
    let shape = egui::epaint::RectShape::filled(rect, CornerRadius::same(18), Color32::WHITE)
        .with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    p.add(shape);
    p.rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, white(26)), StrokeKind::Inside);
}

/// Cartão de vidro: translúcido, borda clara e brilho no topo.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(pal().card)
        .stroke(Stroke::new(1.0, white(22)))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .shadow(Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(if pal().dark { 45 } else { 14 }) })
}

pub fn tinted_card(color: Color32) -> egui::Frame {
    card().fill(color.gamma_multiply(0.14)).stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
}

/// Título pequeno de seção ("VÍDEO", "ÁUDIO"...).
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(title.to_uppercase()).size(11.0).color(text(2)).family(semibold()));
}

// ----------------------------------------------------------------- componentes

/// Controle segmentado com "pílula" deslizante.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, id: &str, value: &mut T, items: &[(T, Option<Icon>, &str)], height: f32) -> Response {
    let width = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let n = items.len().max(1);
    let seg_w = rect.width() / n as f32;
    let idx = items.iter().position(|(v, _, _)| v == value).unwrap_or(0);
    let anim = ui.ctx().animate_value_with_time(ui.scope_id().with(id), idx as f32, 0.18);
    let enabled = ui.is_enabled();
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let rad = CornerRadius::same(((height / 2.0).min(11.0)) as u8);
    let p = ui.painter().clone();
    p.rect(rect, rad, white(10), Stroke::new(1.0, white(16)), StrokeKind::Inside);
    let thumb = Rect::from_min_size(pos2(rect.left() + anim * seg_w, rect.top()), vec2(seg_w, height)).shrink(3.0);
    p.rect(thumb, CornerRadius::same(rad.nw.saturating_sub(3)), fade(pal().thumb), Stroke::new(1.0, fade(white(30))), StrokeKind::Inside);

    for (i, (v, icon, label)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * seg_w, rect.top()), vec2(seg_w, height));
        let ir = ui.interact(r, ui.scope_id().with((id, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if ir.clicked() && *value != *v {
            *value = *v;
            resp.mark_changed();
        }
        let selected = i == idx;
        let col = fade(if selected || ir.hovered() { text(0) } else { text(1) });
        let font = FontId::new((height * 0.4).clamp(12.0, 14.5), if selected { semibold() } else { FontFamily::Proportional });
        let galley = p.layout_no_wrap(label.to_string(), font, col);
        let isz = if icon.is_some() { height * 0.42 } else { 0.0 };
        let gap = if icon.is_some() { 7.0 } else { 0.0 };
        let total = isz + gap + galley.size().x;
        let x0 = r.center().x - total / 2.0;
        if let Some(ic) = icon {
            icons::paint(&p, Rect::from_center_size(pos2(x0 + isz / 2.0, r.center().y), vec2(isz, isz)), *ic, col);
        }
        p.galley(pos2(x0 + isz + gap, r.center().y - galley.size().y / 2.0), galley, col);
    }
    resp
}

/// Chave liga/desliga estilo iOS.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(42.0, 25.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let bg = lerp_color(white(30), accent(), t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(13), bg);
    let x = egui::lerp(rect.left() + 12.5..=rect.right() - 12.5, t);
    p.circle_filled(pos2(x, rect.center().y + 0.8), 10.5, Color32::from_black_alpha(50));
    p.circle_filled(pos2(x, rect.center().y), 10.5, if *on && pal().accent.r() > 200 && pal().accent.g() > 200 { pal().on_accent } else { Color32::WHITE });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Linha "rótulo .......... controle" dentro de um cartão.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        ui.allocate_ui_with_layout(vec2(70.0, 30.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(70.0);
            ui.label(egui::RichText::new(label).color(text(1)));
        });
        add(ui);
    });
}

/// Linha com chave liga/desliga à direita.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> Response {
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        ui.label(egui::RichText::new(label).color(text(0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| toggle(ui, on)).inner
    })
    .inner
}

/// Botão circular de vidro com ícone.
pub fn round_button(ui: &mut Ui, icon: Icon, diameter: f32, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip);
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let fill = lerp_color(white(18), white(34), hover);
    p.circle(rect.center(), diameter / 2.0, fill, Stroke::new(1.0, white(30)));
    let s = diameter * 0.38;
    icons::paint(p, Rect::from_center_size(rect.center(), Vec2::splat(s)), icon, text(0));
    resp
}

/// Botão de pílula (texto). `accent` = preenchido com a cor de destaque.
pub fn pill_button(ui: &mut Ui, label: &str, accent: bool) -> Response {
    let font = FontId::new(14.0, semibold());
    let fg = if accent { pal().on_accent } else { text(0) };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let size = vec2(galley.size().x + 28.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let (fill, stroke) = if accent {
        (lerp_color(pal().accent, pal().accent.gamma_multiply(0.85), hover), Stroke::NONE)
    } else {
        (lerp_color(white(16), white(28), hover), Stroke::new(1.0, white(26)))
    };
    p.rect(rect, CornerRadius::same(16), fill, stroke, StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Botão principal de gravar: anel + círculo vermelho que vira quadrado ao gravar (estilo iOS).
pub fn record_button(ui: &mut Ui, recording: bool, busy: bool) -> Response {
    let d = 78.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let enabled = ui.is_enabled() && !busy;
    let ctx = ui.ctx().clone();
    let t = ctx.animate_bool_with_time(resp.id.with("rec"), recording, 0.28);
    let hover = ctx.animate_bool_with_time(resp.id.with("hov"), resp.hovered() && enabled, 0.12);
    let press = ctx.animate_bool_with_time(resp.id.with("press"), resp.is_pointer_button_down_on() && enabled, 0.08);
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, d / 2.0, white(10));
    p.circle_stroke(c, d / 2.0 - 2.0, Stroke::new(3.0, if enabled { text(0) } else { white(90) }));
    if busy {
        egui::Spinner::new().size(28.0).color(text(0)).paint_at(ui, Rect::from_center_size(c, Vec2::splat(28.0)));
        return resp;
    }
    let size = egui::lerp(58.0..=30.0, t) * (1.0 + 0.04 * hover - 0.06 * press);
    let corner = egui::lerp(size / 2.0..=7.0, t);
    let rec = pal().record;
    let red = if enabled { rec } else { rec.gamma_multiply(0.4) };
    p.rect_filled(Rect::from_center_size(c, Vec2::splat(size)), CornerRadius::same(corner as u8), red);
    resp
}

/// Relógio com dígitos de largura fixa (não "pula" enquanto conta).
pub fn timer(ui: &mut Ui, text_: &str, size: f32, color: Color32) {
    let font = FontId::new(size, thin());
    let p = ui.painter().clone();
    let digit_w = (0..10)
        .map(|d| p.layout_no_wrap(d.to_string(), font.clone(), color).size().x)
        .fold(0.0f32, f32::max);
    let colon_w = p.layout_no_wrap(":".into(), font.clone(), color).size().x + size * 0.08;
    let total: f32 = text_.chars().map(|c| if c == ':' { colon_w } else { digit_w }).sum();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), size * 1.15), Sense::hover());
    let mut x = rect.center().x - total / 2.0;
    for ch in text_.chars() {
        let w = if ch == ':' { colon_w } else { digit_w };
        let dy = if ch == ':' { -size * 0.06 } else { 0.0 };
        p.text(pos2(x + w / 2.0, rect.center().y + dy), Align2::CENTER_CENTER, ch, font.clone(), color);
        x += w;
    }
}

/// Pílula de status com ponto colorido (pulsa quando `pulse`).
pub fn status_pill(ui: &mut Ui, label: &str, color: Color32, pulse: bool) {
    let font = FontId::new(12.5, semibold());
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, text(0));
    let size = vec2(galley.size().x + 34.0, 26.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect(rect, CornerRadius::same(13), white(14), Stroke::new(1.0, white(22)), StrokeKind::Inside);
        let mut dot = color;
        if pulse {
            let t = ui.input(|i| i.time) as f32;
            dot = color.gamma_multiply(0.55 + 0.45 * (t * 4.0).sin().abs());
            ui.ctx().request_repaint();
        }
        p.circle_filled(pos2(rect.left() + 14.0, rect.center().y), 4.0, dot);
        p.galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, text(0));
    });
}

pub enum TitleAction {
    None,
    Close,
    Minimize,
    Right,
}

/// Barra de título própria: botões estilo macOS à esquerda, título no centro e um botão à direita.
pub fn title_bar(ui: &mut Ui, rect: Rect, title: &str, right_icon: Icon, right_tip: &str) -> TitleAction {
    let mut action = TitleAction::None;
    let bar = ui.interact(rect, ui.scope_id().with("titlebar"), Sense::click_and_drag());
    if bar.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    let p = ui.painter().clone();
    p.text(rect.center(), Align2::CENTER_CENTER, title, FontId::new(13.5, semibold()), text(1));

    let group_hover = ui.rect_contains_pointer(Rect::from_min_size(rect.min, vec2(70.0, rect.height())));
    for (i, (color, kind)) in [(Color32::from_rgb(255, 95, 87), 0), (Color32::from_rgb(254, 188, 46), 1)].into_iter().enumerate() {
        let c = pos2(rect.left() + 20.0 + i as f32 * 20.0, rect.center().y);
        let r = ui.interact(Rect::from_center_size(c, Vec2::splat(16.0)), ui.scope_id().with(("tl", i)), Sense::click());
        p.circle_filled(c, 6.5, color);
        if group_hover {
            let s = Stroke::new(1.4, Color32::from_black_alpha(150));
            if kind == 0 {
                p.line_segment([c + vec2(-2.6, -2.6), c + vec2(2.6, 2.6)], s);
                p.line_segment([c + vec2(-2.6, 2.6), c + vec2(2.6, -2.6)], s);
            } else {
                p.line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 0.0)], s);
            }
        }
        if r.clicked() {
            action = if kind == 0 { TitleAction::Close } else { TitleAction::Minimize };
        }
    }

    let rb = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(28.0));
    let r = ui.interact(rb, ui.scope_id().with("tb-right"), Sense::click()).on_hover_text(right_tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() {
        p.circle_filled(rb.center(), 14.0, white(20));
    }
    icons::paint(&p, Rect::from_center_size(rb.center(), Vec2::splat(16.0)), right_icon, if r.hovered() { text(0) } else { text(1) });
    if r.clicked() {
        action = TitleAction::Right;
    }
    action
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Amostras clicáveis dos temas (aplicam na hora).
pub fn theme_picker(ui: &mut Ui, value: &mut ThemeName) -> Response {
    let n = ThemeName::ALL.len() as f32;
    let width = ui.available_width();
    let cell = width / n;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 66.0), Sense::hover());
    for (i, name) in ThemeName::ALL.iter().enumerate() {
        let p = palette(*name);
        let cr = Rect::from_min_size(pos2(rect.left() + i as f32 * cell, rect.top()), vec2(cell, rect.height()));
        let c = pos2(cr.center().x, cr.top() + 22.0);
        let ir = ui
            .interact(Rect::from_center_size(c, Vec2::splat(44.0)), ui.scope_id().with(("theme", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if ir.clicked() && *value != *name {
            *value = *name;
            resp.mark_changed();
        }
        let selected = *value == *name;
        let painter = ui.painter();
        // fundo do tema: cor base misturada com a primeira mancha de luz
        let mix = |a: [f32; 3], b: [f32; 3], k: f32| Color32::from_rgb(
            (a[0] + (b[0] - a[0]) * k) as u8,
            (a[1] + (b[1] - a[1]) * k) as u8,
            (a[2] + (b[2] - a[2]) * k) as u8,
        );
        let base = match p.blobs.first() {
            Some(b) => mix(p.base_top, b.3, 0.45),
            None => mix(p.base_top, p.base_bottom, 0.5),
        };
        if selected {
            painter.circle_stroke(c, 21.5, Stroke::new(2.0, accent()));
        }
        painter.circle(c, 17.0, base, Stroke::new(1.0, white(if ir.hovered() { 70 } else { 40 })));
        painter.circle_filled(c, 7.0, p.accent);
        let col = if selected { text(0) } else { text(1) };
        let font = FontId::new(11.5, if selected { semibold() } else { FontFamily::Proportional });
        painter.text(pos2(c.x, cr.bottom() - 9.0), Align2::CENTER_CENTER, name.label(), font, col);
    }
    resp
}
