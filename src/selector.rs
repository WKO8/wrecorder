//! Seleção de área: tira uma captura da tela, mostra em tela cheia escurecida e o
//! usuário arrasta um retângulo. Roda em um subprocesso (`wrecorder __select`) para
//! que o laço de eventos da janela possa ser criado quantas vezes for preciso.

use crate::capture::Rect;
use anyhow::{anyhow, bail, Context, Result};
use std::num::NonZeroU32;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

pub struct Image {
    pub w: u32,
    pub h: u32,
    /// 0x00RRGGBB
    pub px: Vec<u32>,
}

/// Lê o primeiro quadro de um arquivo YUV4MPEG2 (4:4:4 ou 4:2:0).
pub fn read_y4m(data: &[u8]) -> Result<Image> {
    let nl = data.iter().position(|&b| b == b'\n').ok_or_else(|| anyhow!("y4m sem cabeçalho"))?;
    let header = std::str::from_utf8(&data[..nl])?;
    if !header.starts_with("YUV4MPEG2") {
        bail!("não é YUV4MPEG2");
    }
    let (mut w, mut h, mut c444) = (0usize, 0usize, false);
    for tok in header.split_whitespace() {
        match tok.as_bytes()[0] {
            b'W' => w = tok[1..].parse()?,
            b'H' => h = tok[1..].parse()?,
            b'C' => c444 = tok.starts_with("C444"),
            _ => {}
        }
    }
    let rest = &data[nl + 1..];
    let fnl = rest.iter().position(|&b| b == b'\n').ok_or_else(|| anyhow!("y4m sem quadro"))?;
    let frame = &rest[fnl + 1..];
    let (cw, ch) = if c444 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
    let need = w * h + 2 * cw * ch;
    if w == 0 || h == 0 || frame.len() < need {
        bail!("quadro y4m incompleto");
    }
    let (yp, rest) = frame.split_at(w * h);
    let (up, vp) = rest.split_at(cw * ch);
    let mut px = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let ci = if c444 { y * w + x } else { (y / 2) * cw + x / 2 };
            let yy = (yp[y * w + x] as f32 - 16.0) * 1.164;
            let u = up[ci] as f32 - 128.0;
            let v = vp[ci] as f32 - 128.0;
            let r = (yy + 1.793 * v).round().clamp(0.0, 255.0) as u32;
            let g = (yy - 0.213 * u - 0.533 * v).round().clamp(0.0, 255.0) as u32;
            let b = (yy + 2.112 * u).round().clamp(0.0, 255.0) as u32;
            px[y * w + x] = (r << 16) | (g << 8) | b;
        }
    }
    Ok(Image { w: w as u32, h: h as u32, px })
}

/// Abre o seletor (subprocesso) sobre a imagem `y4m`. Retorna o retângulo em pixels
/// da imagem, ou `None` se o usuário cancelar.
pub fn select_region(y4m: &Path, monitor_pos: Option<(i32, i32)>) -> Result<Option<Rect>> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("__select").arg(y4m);
    if let Some((x, y)) = monitor_pos {
        cmd.arg(format!("{x},{y}"));
    }
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::inherit()).output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(Rect::parse(text.trim()))
}

/// Ponto de entrada do subprocesso `__select`.
pub fn run_selector(y4m: &Path, monitor_pos: Option<(i32, i32)>) -> Result<()> {
    let data = std::fs::read(y4m).with_context(|| format!("lendo {}", y4m.display()))?;
    let img = read_y4m(&data)?;
    let event_loop = EventLoop::new()?;
    let mut app = Selector {
        img,
        monitor_pos,
        window: None,
        surface: None,
        surface_size: (0, 0),
        cache: None,
        history: Vec::new(),
        cursor: (0.0, 0.0),
        drag: None,
        result: None,
    };
    event_loop.run_app(&mut app)?;
    match app.result {
        Some(r) => {
            println!("{},{},{},{}", r.x, r.y, r.w, r.h);
            Ok(())
        }
        None => std::process::exit(1),
    }
}

struct Scaled {
    w: u32,
    h: u32,
    bright: Vec<u32>,
    dim: Vec<u32>,
}

/// Retângulo em pixels da janela (x0,y0 inclusivo; x1,y1 exclusivo).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Area {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Area {
    fn to_damage(self) -> Option<softbuffer::Rect> {
        Some(softbuffer::Rect {
            x: self.x0,
            y: self.y0,
            width: NonZeroU32::new(self.x1.saturating_sub(self.x0))?,
            height: NonZeroU32::new(self.y1.saturating_sub(self.y0))?,
        })
    }
}

const ACCENT: u32 = 0x3d9bff;
const BORDER: u32 = 2;

struct Selector {
    img: Image,
    monitor_pos: Option<(i32, i32)>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    surface_size: (u32, u32),
    cache: Option<Scaled>,
    cursor: (f64, f64),
    drag: Option<(f64, f64)>,
    result: Option<Rect>,
    /// Áreas desenhadas por cima do fundo escuro nos últimos quadros (mais recente primeiro).
    history: Vec<Vec<Area>>,
}

impl Selector {
    fn sel_rect(&self) -> Option<(u32, u32, u32, u32)> {
        let (sx, sy) = self.drag?;
        let (cx, cy) = self.cursor;
        let x0 = sx.min(cx).max(0.0) as u32;
        let y0 = sy.min(cy).max(0.0) as u32;
        let x1 = sx.max(cx).max(0.0) as u32;
        let y1 = sy.max(cy).max(0.0) as u32;
        Some((x0, y0, x1, y1))
    }

    /// O que deve aparecer por cima do fundo escuro neste quadro.
    fn overlay(&self, w: u32, h: u32) -> Vec<Area> {
        if let Some((x0, y0, x1, y1)) = self.sel_rect() {
            let a = Area { x0: x0.min(w - 1), y0: y0.min(h - 1), x1: (x1 + 1).min(w), y1: (y1 + 1).min(h) };
            vec![a]
        } else {
            let (cx, cy) = (self.cursor.0.max(0.0) as u32, self.cursor.1.max(0.0) as u32);
            if cx >= w || cy >= h {
                return Vec::new();
            }
            vec![Area { x0: 0, y0: cy, x1: w, y1: cy + 1 }, Area { x0: cx, y0: 0, x1: cx + 1, y1: h }]
        }
    }

    fn draw(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let size = window.inner_size();
        let (w, h) = (size.width, size.height);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        let overlay = self.overlay(w, h);
        let Some(surface) = self.surface.as_mut() else { return };
        let mut full = false;
        if self.surface_size != (w, h) {
            if surface.resize(nw, nh).is_err() {
                return;
            }
            self.surface_size = (w, h);
            self.history.clear();
            full = true;
        }
        if self.cache.as_ref().map(|c| (c.w, c.h)) != Some((w, h)) {
            // Escala a captura para o tamanho da janela uma única vez.
            let mut bright = vec![0u32; (w * h) as usize];
            let xs: Vec<u32> = (0..w).map(|x| (x as u64 * self.img.w as u64 / w as u64) as u32).collect();
            for y in 0..h {
                let sy = (y as u64 * self.img.h as u64 / h as u64) as u32;
                let src = &self.img.px[(sy * self.img.w) as usize..((sy + 1) * self.img.w) as usize];
                let dst = &mut bright[(y * w) as usize..((y + 1) * w) as usize];
                for (d, &sx) in dst.iter_mut().zip(&xs) {
                    *d = src[sx as usize];
                }
            }
            let dim = bright.iter().map(|&p| ((p >> 2) & 0x3f3f3f) + ((p >> 3) & 0x1f1f1f)).collect();
            self.cache = Some(Scaled { w, h, bright, dim });
            full = true;
        }
        let cache = self.cache.as_ref().unwrap();
        let Ok(mut buf) = surface.buffer_mut() else { return };

        // O buffer recebido guarda o conteúdo de `age` quadros atrás (0 = indefinido).
        let age = buf.age() as usize;
        let full = full || age == 0 || age > self.history.len();
        let copy_rows = |buf: &mut [u32], src: &[u32], a: Area| {
            for y in a.y0..a.y1 {
                let (s, e) = ((y * w + a.x0) as usize, (y * w + a.x1) as usize);
                buf[s..e].copy_from_slice(&src[s..e]);
            }
        };
        if full {
            buf.copy_from_slice(&cache.dim);
        } else {
            // Apaga o que foi desenhado desde aquele quadro.
            for a in self.history[..age].iter().flatten() {
                copy_rows(&mut buf, &cache.dim, *a);
            }
        }

        // Desenha o quadro atual.
        if self.drag.is_some() {
            if let Some(&a) = overlay.first() {
                copy_rows(&mut buf, &cache.bright, a);
                for y in a.y0..a.y1 {
                    let row = (y * w) as usize;
                    if y < a.y0 + BORDER || y + BORDER >= a.y1 {
                        buf[row + a.x0 as usize..row + a.x1 as usize].fill(ACCENT);
                    } else {
                        let left = (a.x0 + BORDER).min(a.x1);
                        let right = a.x1.saturating_sub(BORDER).max(a.x0);
                        buf[row + a.x0 as usize..row + left as usize].fill(ACCENT);
                        buf[row + right as usize..row + a.x1 as usize].fill(ACCENT);
                    }
                }
            }
        } else {
            for a in &overlay {
                for y in a.y0..a.y1 {
                    buf[(y * w + a.x0) as usize..(y * w + a.x1) as usize].fill(ACCENT);
                }
            }
        }

        // Diz ao compositor só o que mudou em relação ao quadro anterior.
        let damage: Vec<softbuffer::Rect> = if full {
            vec![softbuffer::Rect { x: 0, y: 0, width: nw, height: nh }]
        } else {
            self.history.first().into_iter().flatten().chain(overlay.iter()).filter_map(|a| a.to_damage()).collect()
        };
        window.pre_present_notify();
        if damage.is_empty() {
            let _ = buf.present();
        } else {
            let _ = buf.present_with_damage(&damage);
        }
        self.history.insert(0, overlay);
        self.history.truncate(4);
    }
}

impl ApplicationHandler for Selector {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let monitor = self.monitor_pos.and_then(|(x, y)| {
            el.available_monitors().find(|m| m.position().x == x && m.position().y == y)
        });
        let attrs = Window::default_attributes()
            .with_title("wrecorder — selecione a área (Esc cancela)")
            .with_decorations(false)
            .with_fullscreen(Some(Fullscreen::Borderless(monitor)));
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("erro criando janela: {e}");
                el.exit();
                return;
            }
        };
        window.set_cursor(CursorIcon::Crosshair);
        let ctx = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&ctx, window.clone()).expect("softbuffer surface"));
        window.focus_window();
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if event.logical_key == Key::Named(NamedKey::Escape) {
                    el.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => self.drag = Some(self.cursor),
                (MouseButton::Left, ElementState::Released) => {
                    if let (Some((x0, y0, x1, y1)), Some(win)) = (self.sel_rect(), self.window.as_ref()) {
                        let size = win.inner_size();
                        // Limita ao tamanho da janela antes de converter para pixels da imagem.
                        let (ww, wh) = (size.width.max(1), size.height.max(1));
                        let (x0, x1) = (x0.min(ww), x1.min(ww));
                        let (y0, y1) = (y0.min(wh), y1.min(wh));
                        let sx = self.img.w as f64 / ww as f64;
                        let sy = self.img.h as f64 / wh as f64;
                        let px = ((x0 as f64 * sx).round() as u32).min(self.img.w);
                        let py = ((y0 as f64 * sy).round() as u32).min(self.img.h);
                        let r = Rect {
                            x: px as i32,
                            y: py as i32,
                            w: ((x1 as f64 * sx).round() as u32).min(self.img.w) - px,
                            h: ((y1 as f64 * sy).round() as u32).min(self.img.h) - py,
                        };
                        if r.w >= 16 && r.h >= 16 {
                            self.result = Some(r);
                            el.exit();
                            return;
                        }
                    }
                    self.drag = None;
                }
                (MouseButton::Right, ElementState::Pressed) => el.exit(),
                _ => {}
            },
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_y4m_444_and_420() {
        // 2x2 4:4:4: branco, preto, cinza, branco (U=V=128)
        let mut d = b"YUV4MPEG2 W2 H2 F30:1 Ip A1:1 C444\nFRAME\n".to_vec();
        d.extend([235, 16, 126, 235]);
        d.extend([128; 4]);
        d.extend([128; 4]);
        let img = read_y4m(&d).unwrap();
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(img.px[0], 0xffffff);
        assert_eq!(img.px[1], 0x000000);

        // 4:2:0 com dimensões ímpares (chroma arredondado para cima)
        let mut d = b"YUV4MPEG2 W3 H3 F30:1 Ip C420jpeg\nFRAME\n".to_vec();
        d.extend([235; 9]);
        d.extend([128; 8]);
        let img = read_y4m(&d).unwrap();
        assert!(img.px.iter().all(|&p| p == 0xffffff));
        assert!(read_y4m(b"YUV4MPEG2 W3 H3\nFRAME\n\x00").is_err());
    }

    /// Arquivo real do GStreamer: `WRECORDER_Y4M=arquivo.y4m cargo test`.
    #[test]
    fn decodes_real_file() {
        let Ok(path) = std::env::var("WRECORDER_Y4M") else { return };
        let img = read_y4m(&std::fs::read(path).unwrap()).unwrap();
        assert!(img.w > 0 && img.h > 0);
        assert!(img.px.iter().any(|&p| p != 0), "imagem toda preta");
        eprintln!("ok: {}x{}", img.w, img.h);
    }

    #[test]
    fn parses_rect() {
        assert_eq!(Rect::parse("10,20,640,480"), Some(Rect { x: 10, y: 20, w: 640, h: 480 }));
        assert_eq!(Rect::parse("1,2,0,4"), None);
        assert_eq!(Rect { x: 0, y: 0, w: 641, h: 481 }.even(), Rect { x: 0, y: 0, w: 640, h: 480 });
    }
}
