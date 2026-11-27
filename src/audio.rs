//! Entradas de áudio para o ffmpeg.
//!
//! * Linux: o próprio ffmpeg lê do PulseAudio/PipeWire (`-f pulse`). O áudio do
//!   sistema vem do "monitor" da saída padrão.
//! * Windows: capturamos com WASAPI (cpal) — inclusive o loopback do áudio do
//!   sistema — e enviamos PCM float para o ffmpeg por um socket TCP local.

use crate::config::AudioConfig;
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    Mic,
    System,
}

/// Uma entrada de áudio já pronta: argumentos de input do ffmpeg + recurso vivo
/// (no Windows, a thread de captura).
pub struct AudioInput {
    pub args: Vec<String>,
    #[cfg(windows)]
    #[allow(dead_code)]
    pub feeder: win::Feeder,
}

#[cfg(target_os = "linux")]
pub fn open(kind: AudioKind, cfg: &AudioConfig) -> Result<AudioInput> {
    let device = match kind {
        AudioKind::Mic if !cfg.mic_device.trim().is_empty() => cfg.mic_device.trim().to_string(),
        AudioKind::Mic => "default".to_string(),
        AudioKind::System if !cfg.system_device.trim().is_empty() => cfg.system_device.trim().to_string(),
        AudioKind::System => "@DEFAULT_MONITOR@".to_string(),
    };
    let args = [
        "-thread_queue_size", "4096",
        "-f", "pulse",
        "-fragment_size", "4096",
        "-i", &device,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    Ok(AudioInput { args })
}

#[cfg(windows)]
pub fn open(kind: AudioKind, cfg: &AudioConfig) -> Result<AudioInput> {
    let feeder = win::Feeder::start(kind, cfg)?;
    let args = vec![
        "-thread_queue_size".into(), "4096".into(),
        "-f".into(), "f32le".into(),
        "-ar".into(), feeder.rate.to_string(),
        "-ac".into(), feeder.channels.to_string(),
        "-i".into(), format!("tcp://127.0.0.1:{}?listen=1", feeder.port),
    ];
    Ok(AudioInput { args, feeder })
}

#[cfg(windows)]
pub mod win {
    use super::AudioKind;
    use crate::config::AudioConfig;
    use anyhow::{anyhow, Context, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SizedSample};
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    pub struct Feeder {
        pub port: u16,
        pub rate: u32,
        pub channels: u16,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for Feeder {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn device_name(d: &cpal::Device) -> String {
        d.description().map(|d| d.name().to_string()).unwrap_or_default()
    }

    fn pick_device(kind: AudioKind, wanted: &str) -> Result<cpal::Device> {
        let host = cpal::default_host();
        let wanted = wanted.trim().to_lowercase();
        if !wanted.is_empty() {
            let devices: Vec<cpal::Device> = match kind {
                AudioKind::Mic => host.input_devices()?.collect(),
                AudioKind::System => host.output_devices()?.collect(),
            };
            if let Some(d) = devices.into_iter().find(|d| device_name(d).to_lowercase().contains(&wanted)) {
                return Ok(d);
            }
            eprintln!("Aviso: dispositivo '{wanted}' não encontrado; usando o padrão.");
        }
        match kind {
            AudioKind::Mic => host.default_input_device().ok_or_else(|| anyhow!("nenhum microfone encontrado")),
            // Um dispositivo de SAÍDA aberto como entrada = loopback WASAPI.
            AudioKind::System => host.default_output_device().ok_or_else(|| anyhow!("nenhuma saída de áudio encontrada")),
        }
    }

    impl Feeder {
        pub fn start(kind: AudioKind, cfg: &AudioConfig) -> Result<Feeder> {
            let wanted = match kind {
                AudioKind::Mic => cfg.mic_device.clone(),
                AudioKind::System => cfg.system_device.clone(),
            };
            let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
            let stop = Arc::new(AtomicBool::new(false));
            let (info_tx, info_rx) = mpsc::channel::<Result<(u32, u16)>>();
            let stop2 = stop.clone();
            let handle = thread::Builder::new().name("audio-capture".into()).spawn(move || {
                if let Err(e) = run(kind, &wanted, port, stop2, &info_tx) {
                    let _ = info_tx.send(Err(e));
                }
            })?;
            let (rate, channels) = info_rx
                .recv_timeout(Duration::from_secs(5))
                .context("dispositivo de áudio não respondeu")??;
            Ok(Feeder { port, rate, channels, stop, handle: Some(handle) })
        }
    }

    fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, tx: mpsc::Sender<Vec<f32>>) -> Result<cpal::Stream>
    where
        T: SizedSample + Sample,
        f32: FromSample<T>,
    {
        Ok(device.build_input_stream::<T, _, _>(
            config.clone(),
            move |data: &[T], _| {
                let _ = tx.send(data.iter().map(|s| f32::from_sample(*s)).collect());
            },
            |e| eprintln!("erro de áudio: {e}"),
            None,
        )?)
    }

    fn run(
        kind: AudioKind,
        wanted: &str,
        port: u16,
        stop: Arc<AtomicBool>,
        info_tx: &mpsc::Sender<Result<(u32, u16)>>,
    ) -> Result<()> {
        let device = pick_device(kind, wanted)?;
        let supported = match kind {
            AudioKind::Mic => device.default_input_config()?,
            AudioKind::System => device.default_output_config()?,
        };
        let config: cpal::StreamConfig = supported.config();
        let rate = config.sample_rate;
        let channels = config.channels;
        info_tx.send(Ok((rate, channels))).ok();

        // Espera o ffmpeg abrir o socket (ele escuta com ?listen=1).
        let mut sock = loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(_) => thread::sleep(Duration::from_millis(15)),
            }
        };
        sock.set_nodelay(true).ok();

        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, tx)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, tx)?,
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, tx)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, tx)?,
            other => return Err(anyhow!("formato de amostra não suportado: {other:?}")),
        };
        stream.play()?;

        // O loopback do WASAPI não entrega nada enquanto não há som tocando.
        // Preenchemos com silêncio para manter o áudio sincronizado com o vídeo.
        let start = Instant::now();
        let mut written: u64 = 0; // em quadros (amostras por canal)
        let mut last_data = Instant::now();
        let mut bytes = Vec::with_capacity(16384);
        while !stop.load(Ordering::SeqCst) {
            bytes.clear();
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(samples) => {
                    last_data = Instant::now();
                    written += (samples.len() / channels as usize) as u64;
                    for s in samples {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let expected = (start.elapsed().as_secs_f64() * rate as f64) as u64;
                    let lag = expected.saturating_sub(written);
                    if last_data.elapsed() > Duration::from_millis(40) && lag > rate as u64 / 50 {
                        let pad = lag - rate as u64 / 100;
                        written += pad;
                        bytes.resize((pad * channels as u64 * 4) as usize, 0);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if !bytes.is_empty() && sock.write_all(&bytes).is_err() {
                break; // ffmpeg encerrou
            }
        }
        drop(stream);
        Ok(())
    }
}
