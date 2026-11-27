//! Configuração persistente (TOML) do wrecorder.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Wav,
    Mp3,
}

impl AudioFormat {
    pub fn ext(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    /// Sem áudio
    None,
    /// Microfone (entrada padrão)
    Mic,
    /// Áudio do sistema (o que sai nos alto-falantes)
    System,
    /// Microfone + áudio do sistema mixados
    Both,
}

impl AudioSource {
    pub fn label(self) -> &'static str {
        match self {
            AudioSource::None => "Sem áudio",
            AudioSource::Mic => "Microfone",
            AudioSource::System => "Áudio do sistema",
            AudioSource::Both => "Microfone + áudio do sistema",
        }
    }
    pub fn has_mic(self) -> bool {
        matches!(self, AudioSource::Mic | AudioSource::Both)
    }
    pub fn has_system(self) -> bool {
        matches!(self, AudioSource::System | AudioSource::Both)
    }
}

/// Redução de ruído aplicada ao microfone (filtros do ffmpeg).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum NoiseReduction {
    Off,
    Light,
    Strong,
}

impl NoiseReduction {
    pub fn label(self) -> &'static str {
        match self {
            NoiseReduction::Off => "Desligada",
            NoiseReduction::Light => "Leve (chiado)",
            NoiseReduction::Strong => "Forte (ambiente barulhento)",
        }
    }

    /// Cadeia de filtros do ffmpeg:
    /// * highpass corta ronco/vibração grave (ventoinha, mesa);
    /// * afftdn remove ruído de fundo constante (chiado), rastreando o perfil do ruído;
    /// * agate (forte) abaixa o que sobra nos silêncios entre as falas.
    pub fn filter(self) -> Option<&'static str> {
        match self {
            NoiseReduction::Off => None,
            NoiseReduction::Light => Some("highpass=f=80,afftdn=nr=12:nf=-50:tn=1"),
            NoiseReduction::Strong => Some(
                "highpass=f=90,afftdn=nr=24:nf=-40:tn=1,agate=threshold=0.02:ratio=2:range=0.25:attack=10:release=250",
            ),
        }
    }
}

/// Tema de cores da janela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Red,
    Green,
    Yellow,
    Blue,
    Black,
    White,
}

impl ThemeName {
    pub const ALL: [ThemeName; 6] =
        [ThemeName::Red, ThemeName::Green, ThemeName::Yellow, ThemeName::Blue, ThemeName::Black, ThemeName::White];

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Red => "Vermelho",
            ThemeName::Green => "Hacker",
            ThemeName::Yellow => "Amarelo",
            ThemeName::Blue => "Azul",
            ThemeName::Black => "Preto",
            ThemeName::White => "Claro",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Inicia a gravação (quando aguardando).
    pub start: String,
    /// Pausa / retoma.
    pub pause: String,
    /// Encerra e salva.
    pub stop: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            start: "Ctrl+Alt+R".into(),
            pause: "Ctrl+Alt+P".into(),
            stop: "Ctrl+Alt+S".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    /// Quadros por segundo.
    pub fps: u32,
    /// Encoder do ffmpeg: "auto", "libx264", "h264_nvenc", "h264_qsv", "h264_amf", ...
    pub encoder: String,
    /// Qualidade (estilo CRF): menor = melhor/maior. 18 ≈ visualmente sem perdas, 23 = padrão, 28 = econômico.
    pub quality: u32,
    /// Preset do libx264 (ultrafast, superfast, veryfast, faster, fast, medium...).
    pub x264_preset: String,
    /// Desenhar o cursor do mouse.
    pub show_cursor: bool,
    /// Fonte de áudio padrão para vídeos.
    pub audio: AudioSource,
    /// Bitrate AAC (kbps) do áudio do vídeo.
    pub audio_bitrate: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            encoder: "auto".into(),
            quality: 23,
            x264_preset: "veryfast".into(),
            show_cursor: true,
            audio: AudioSource::System,
            audio_bitrate: 192,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Formato padrão da gravação somente de áudio.
    pub format: AudioFormat,
    /// Fonte padrão da gravação somente de áudio.
    pub source: AudioSource,
    /// Bitrate do MP3 (kbps).
    pub mp3_bitrate: u32,
    /// Taxa de amostragem de saída.
    pub sample_rate: u32,
    /// Redução de ruído do microfone: "off", "light" ou "strong".
    pub noise_reduction: NoiseReduction,
    /// Dispositivo de microfone (vazio = padrão do sistema).
    /// Linux: nome da fonte PulseAudio/PipeWire. Windows: parte do nome do dispositivo.
    pub mic_device: String,
    /// Dispositivo do áudio do sistema (vazio = padrão).
    /// Linux: nome do monitor PulseAudio/PipeWire. Windows: parte do nome da saída.
    pub system_device: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            format: AudioFormat::Mp3,
            source: AudioSource::Mic,
            mp3_bitrate: 192,
            sample_rate: 48000,
            noise_reduction: NoiseReduction::Light,
            mic_device: String::new(),
            system_device: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Pasta de saída (vazio = Vídeos/Música do usuário).
    pub output_dir: String,
    /// Começar a gravar imediatamente, sem esperar o atalho de início (modo terminal).
    pub start_immediately: bool,
    /// Minimizar a janela do WRecorder ao começar a gravar (para ela não aparecer no vídeo).
    pub minimize_on_record: bool,
    /// Caminho do executável do ffmpeg.
    pub ffmpeg: String,
    /// Caminho do gst-launch-1.0 (usado apenas no Linux/Wayland).
    pub gst_launch: String,
    /// Backend de captura no Linux: "auto", "x11" ou "wayland".
    pub linux_backend: String,
    /// Tema de cores: red, green, yellow, blue, black, white.
    pub theme: ThemeName,
    pub hotkeys: Hotkeys,
    pub video: VideoConfig,
    pub audio: AudioConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            start_immediately: false,
            minimize_on_record: true,
            ffmpeg: "ffmpeg".into(),
            gst_launch: "gst-launch-1.0".into(),
            linux_backend: "auto".into(),
            theme: ThemeName::Red,
            hotkeys: Hotkeys::default(),
            video: VideoConfig::default(),
            audio: AudioConfig::default(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wrecorder")
            .join("config.toml")
    }

    /// Carrega a configuração; cria o arquivo com os valores padrão se não existir.
    pub fn load_or_create() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save().ok();
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("lendo {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("config inválida em {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = toml::to_string_pretty(self)?;
        let text = format!(
            "# Configuração do wrecorder\n\
             # Atalhos: combine Ctrl, Alt, Shift, Super com uma tecla (A-Z, 0-9, F1-F24...).\n\
             # Exemplos: \"Ctrl+Alt+R\", \"Shift+F9\", \"Super+Shift+S\".\n\n{body}"
        );
        std::fs::write(&path, text).with_context(|| format!("salvando {}", path.display()))
    }

    pub fn output_dir(&self, video: bool) -> PathBuf {
        if !self.output_dir.trim().is_empty() {
            return PathBuf::from(self.output_dir.trim());
        }
        let base = if video { dirs::video_dir() } else { dirs::audio_dir() };
        base.or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wrecorder")
    }
}
