//! Atalhos globais (funcionam mesmo com o terminal sem foco).
//!
//! * X11 e Windows: crate `global-hotkey`.
//! * Wayland: portal GlobalShortcuts (GNOME 48+, KDE). O sistema pode pedir
//!   confirmação na primeira vez e permite remapear os atalhos nas configurações.
//! * Sempre: teclas no próprio terminal (Enter / Espaço / Q).

use crate::config::Hotkeys;
use anyhow::{anyhow, Result};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Pause,
    Stop,
}

pub struct HotkeyService {
    /// Descrição de como os atalhos ficaram (para mostrar ao usuário).
    pub status: String,
    _backend: Backend,
}

#[allow(dead_code)]
enum Backend {
    None,
    Native(global_hotkey::GlobalHotKeyManager),
    #[cfg(target_os = "linux")]
    Portal(std::thread::JoinHandle<()>),
}

impl HotkeyService {
    /// Registra os atalhos; os eventos vão para `tx`.
    pub fn new(keys: &Hotkeys, wayland: bool, tx: Sender<Action>) -> HotkeyService {
        let (backend, status) = if wayland {
            #[cfg(target_os = "linux")]
            {
                match portal::start(keys, tx) {
                    Ok(h) => (Backend::Portal(h), format!(
                        "atalhos globais via portal do sistema ({} / {} / {})",
                        keys.start, keys.pause, keys.stop
                    )),
                    Err(e) => (Backend::None, format!("atalhos globais indisponíveis no Wayland: {e:#}")),
                }
            }
            #[cfg(not(target_os = "linux"))]
            (Backend::None, String::new())
        } else {
            match native(keys, tx) {
                Ok(m) => (Backend::Native(m), format!(
                    "atalhos globais ativos ({} / {} / {})",
                    keys.start, keys.pause, keys.stop
                )),
                Err(e) => (Backend::None, format!("atalhos globais indisponíveis: {e:#}")),
            }
        };
        HotkeyService { status, _backend: backend }
    }
}

fn parse(s: &str) -> Result<global_hotkey::hotkey::HotKey> {
    let norm = s
        .split('+')
        .map(|p| match p.trim().to_lowercase().as_str() {
            "win" | "windows" | "logo" | "meta" => "super".to_string(),
            "control" => "ctrl".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");
    norm.parse().map_err(|e| anyhow!("atalho inválido '{s}': {e}"))
}

/// Valida a sintaxe dos atalhos configurados.
pub fn validate(keys: &Hotkeys) -> Result<()> {
    parse(&keys.start)?;
    parse(&keys.pause)?;
    parse(&keys.stop)?;
    Ok(())
}

fn native(keys: &Hotkeys, tx: Sender<Action>) -> Result<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    let start = parse(&keys.start)?;
    let pause = parse(&keys.pause)?;
    let stop = parse(&keys.stop)?;
    let manager = GlobalHotKeyManager::new().map_err(|e| anyhow!("{e}"))?;
    for hk in [start, pause, stop] {
        manager.register(hk).map_err(|e| anyhow!("registrando {hk}: {e}"))?;
    }
    let (ids, tx) = ((start.id(), pause.id(), stop.id()), std::sync::Mutex::new(tx));
    GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
        if ev.state() != HotKeyState::Pressed {
            return;
        }
        let action = match ev.id() {
            id if id == ids.0 => Action::Start,
            id if id == ids.1 => Action::Pause,
            id if id == ids.2 => Action::Stop,
            _ => return,
        };
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(action);
        }
    }));
    Ok(manager)
}

/// No Windows os atalhos chegam como mensagens na fila da thread que os registrou.
pub fn pump_messages() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(target_os = "linux")]
mod portal {
    use super::*;
    use std::sync::mpsc;
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    /// "Ctrl+Alt+R" -> "CTRL+ALT+r" (formato da especificação XDG shortcuts).
    fn to_xdg(s: &str) -> String {
        s.split('+')
            .map(|p| {
                let p = p.trim();
                match p.to_lowercase().as_str() {
                    "ctrl" | "control" => "CTRL".to_string(),
                    "alt" => "ALT".to_string(),
                    "shift" => "SHIFT".to_string(),
                    "super" | "win" | "logo" | "meta" => "LOGO".to_string(),
                    k if k.len() == 1 => k.to_string(),
                    _ => p.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("+")
    }

    pub fn start(keys: &Hotkeys, tx: Sender<Action>) -> Result<std::thread::JoinHandle<()>> {
        let shortcuts = vec![
            NewShortcut::new("start", "wrecorder: iniciar gravação").preferred_trigger(to_xdg(&keys.start).as_str()),
            NewShortcut::new("pause", "wrecorder: pausar / retomar").preferred_trigger(to_xdg(&keys.pause).as_str()),
            NewShortcut::new("stop", "wrecorder: encerrar gravação").preferred_trigger(to_xdg(&keys.stop).as_str()),
        ];
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let handle = std::thread::Builder::new().name("portal-shortcuts".into()).spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.into()));
                    return;
                }
            };
            rt.block_on(async move {
                let setup = async {
                    let proxy = GlobalShortcuts::new().await?;
                    let session = proxy.create_session(Default::default()).await?;
                    let stream = proxy.receive_activated().await?;
                    proxy
                        .bind_shortcuts(&session, &shortcuts, None, Default::default())
                        .await?
                        .response()?;
                    anyhow::Ok((proxy, session, stream))
                };
                let (_proxy, _session, mut stream) = match tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    setup,
                )
                .await
                {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                    Err(_) => {
                        let _ = ready_tx.send(Err(anyhow!("o portal não respondeu")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                while let Some(ev) = stream.next().await {
                    let action = match ev.shortcut_id() {
                        "start" => Action::Start,
                        "pause" => Action::Pause,
                        "stop" => Action::Stop,
                        _ => continue,
                    };
                    if tx.send(action).is_err() {
                        break;
                    }
                }
            });
        })?;
        ready_rx.recv().map_err(|_| anyhow!("thread do portal terminou"))??;
        Ok(handle)
    }
}
