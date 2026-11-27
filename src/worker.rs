//! Thread que executa as gravações para a interface gráfica, para a janela nunca travar
//! (o diálogo do portal, a seleção de área e o encerramento do ffmpeg são bloqueantes).

use crate::config::{AudioFormat, AudioSource, Config};
use crate::hotkeys::Action;
use crate::prepare::{self, Backend, TargetSpec};
use crate::recorder::Recorder;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Esperando o usuário escolher no portal / selecionar a área.
    Preparing,
    /// Janela minimizando; a gravação começa em instantes.
    Countdown,
    Recording,
    Paused,
    Finishing,
}

/// Pedido de gravação (montado a partir do que está selecionado na janela).
#[derive(Clone, Debug)]
pub struct Request {
    pub cfg: Config,
    /// `None` = somente áudio.
    pub target: Option<TargetSpec>,
    pub audio: AudioSource,
    pub format: AudioFormat,
}

pub enum Msg {
    /// Ação vinda de um botão ou atalho global.
    Action(Action),
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    pub elapsed: Duration,
    pub last_file: Option<PathBuf>,
    pub error: Option<String>,
    /// O que gravar quando o atalho de início for pressionado (a GUI mantém atualizado).
    pub request: Option<Request>,
}

pub type Shared = Arc<Mutex<Status>>;

pub fn spawn(backend: Backend, repaint: impl Fn() + Send + 'static) -> (Sender<Msg>, Shared) {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Status {
        phase: Phase::Idle,
        elapsed: Duration::ZERO,
        last_file: None,
        error: None,
        request: None,
    }));
    let sh = shared.clone();
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || run(rx, sh, backend, repaint))
        .expect("thread do gravador");
    (tx, shared)
}

fn set_phase(shared: &Shared, phase: Phase, repaint: &dyn Fn()) {
    shared.lock().unwrap().phase = phase;
    repaint();
}

fn run(rx: Receiver<Msg>, shared: Shared, backend: Backend, repaint: impl Fn()) {
    let mut rec: Option<Recorder> = None;
    loop {
        let msg = rx.recv_timeout(Duration::from_millis(100));
        let phase = shared.lock().unwrap().phase;
        match msg {
            Ok(Msg::Action(action)) => match (phase, action) {
                (Phase::Idle, Action::Start) | (Phase::Idle, Action::Pause) => {
                    let req = shared.lock().unwrap().request.clone();
                    if let Some(req) = req {
                        rec = start(req, backend, &shared, &repaint);
                    }
                }
                (Phase::Recording, Action::Pause) => {
                    if let Some(r) = rec.as_mut() {
                        if let Err(e) = r.pause() {
                            shared.lock().unwrap().error = Some(format!("{e:#}"));
                        }
                    }
                    set_phase(&shared, Phase::Paused, &repaint);
                }
                (Phase::Paused, Action::Pause) | (Phase::Paused, Action::Start) => {
                    if let Some(r) = rec.as_mut() {
                        match r.start() {
                            Ok(()) => set_phase(&shared, Phase::Recording, &repaint),
                            Err(e) => shared.lock().unwrap().error = Some(format!("{e:#}")),
                        }
                    }
                }
                (Phase::Recording | Phase::Paused, Action::Stop) => {
                    finish(rec.take(), &shared, &repaint);
                }
                _ => {}
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(r) = rec.take() {
                    let _ = r.finish();
                }
                return;
            }
        }

        if let Some(r) = rec.as_mut() {
            if let Err(e) = r.check_health() {
                shared.lock().unwrap().error = Some(format!("{e:#}"));
                finish(rec.take(), &shared, &repaint);
                continue;
            }
            shared.lock().unwrap().elapsed = r.elapsed();
            repaint();
        }
    }
}

fn start(req: Request, backend: Backend, shared: &Shared, repaint: &dyn Fn()) -> Option<Recorder> {
    {
        let mut s = shared.lock().unwrap();
        s.error = None;
        s.elapsed = Duration::ZERO;
    }
    set_phase(shared, Phase::Preparing, repaint);
    let fail = |e: anyhow::Error| {
        let mut s = shared.lock().unwrap();
        s.error = Some(format!("{e:#}"));
        s.phase = Phase::Idle;
        repaint();
        None
    };
    let cfg = req.cfg;
    if let Err(e) = crate::ffmpeg::check_available(&cfg.ffmpeg) {
        return fail(e);
    }
    let plan = match req.target {
        Some(spec) => match prepare::make_capture(&cfg, backend, spec) {
            Ok(Some(capture)) => prepare::video_plan(&cfg, capture, req.audio, None),
            Ok(None) => {
                set_phase(shared, Phase::Idle, repaint);
                return None;
            }
            Err(e) => return fail(e),
        },
        None => {
            if req.audio == AudioSource::None {
                return fail(anyhow::anyhow!("escolha uma fonte de áudio"));
            }
            prepare::audio_plan(&cfg, req.format, req.audio, None)
        }
    };
    let video = plan.video.is_some();
    let mut rec = match Recorder::new(plan, cfg.clone()) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    if video && cfg.minimize_on_record {
        // Dá tempo da janela sair da tela antes do primeiro quadro.
        set_phase(shared, Phase::Countdown, repaint);
        std::thread::sleep(Duration::from_millis(700));
    }
    match rec.start() {
        Ok(()) => {
            set_phase(shared, Phase::Recording, repaint);
            Some(rec)
        }
        Err(e) => {
            rec.discard();
            fail(e)
        }
    }
}

fn finish(rec: Option<Recorder>, shared: &Shared, repaint: &dyn Fn()) {
    let Some(rec) = rec else {
        set_phase(shared, Phase::Idle, repaint);
        return;
    };
    set_phase(shared, Phase::Finishing, repaint);
    let result = if rec.has_content() { rec.finish() } else { Err(anyhow::anyhow!("nada foi gravado")) };
    let mut s = shared.lock().unwrap();
    match result {
        Ok(path) => s.last_file = Some(path),
        Err(e) => {
            let prev = s.error.take().map(|p| format!("{p}\n")).unwrap_or_default();
            s.error = Some(format!("{prev}{e:#}"));
        }
    }
    s.phase = Phase::Idle;
    drop(s);
    repaint();
}
