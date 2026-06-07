use crate::config::{Config, ConfiguredTts};
use crate::logs;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const FFMPEG_BIN: &str = "ffmpeg";
const ELEVENLABS_API_KEY_ENV: &str = "ELEVENLABS_API_KEY";
const ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io";
const ELEVENLABS_OUTPUT_FORMAT: &str = "opus_48000_128";
const VOICEBOX_BASE_URL: &str = "http://127.0.0.1:17493";
const VOICEBOX_APP_RELATIVE_PATH: &str = "Voicebox.app/Contents/MacOS/Voicebox";
const VOICEBOX_PROFILE_ID: &str = "60897a97-bbe7-4e3f-82f2-f27e8c377d30";
const VOICEBOX_ENGINE: &str = "chatterbox_turbo";
const VOICEBOX_SPEED: Option<f64> = Some(1.5);
const LOCAL_TTS_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_TTS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const VOICEBOX_STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const VOICEBOX_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(500);
const VOICEBOX_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
struct TtsRuntime {
    elevenlabs_base_url: String,
    elevenlabs_api_key: Option<String>,
    voicebox: VoiceboxRuntime,
}

impl TtsRuntime {
    fn from_config(cfg: &Config) -> Self {
        Self {
            elevenlabs_base_url: ELEVENLABS_BASE_URL.to_string(),
            elevenlabs_api_key: std::env::var(ELEVENLABS_API_KEY_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            voicebox: VoiceboxRuntime::from_config(cfg),
        }
    }
}

#[derive(Debug, Clone)]
struct VoiceboxRuntime {
    base_url: String,
    generations_dir: PathBuf,
    ffmpeg: PathBuf,
    executable: PathBuf,
    model_cache_dir: PathBuf,
    data_dir: PathBuf,
    startup_timeout: Duration,
    poll_interval: Duration,
    speed: Option<f64>,
}

impl VoiceboxRuntime {
    fn from_config(cfg: &Config) -> Self {
        let voicebox_dir = cfg.xdg_data_home.join("gateway/voicebox");
        let data_dir = voicebox_dir.join("data");
        Self {
            base_url: VOICEBOX_BASE_URL.to_string(),
            generations_dir: data_dir.join("generations"),
            ffmpeg: PathBuf::from(FFMPEG_BIN),
            executable: voicebox_dir.join(VOICEBOX_APP_RELATIVE_PATH),
            model_cache_dir: voicebox_dir.join("models"),
            data_dir,
            startup_timeout: VOICEBOX_STARTUP_TIMEOUT,
            poll_interval: VOICEBOX_STARTUP_POLL_INTERVAL,
            speed: VOICEBOX_SPEED,
        }
    }
}

#[derive(Debug)]
pub(crate) struct VoiceOutput {
    path: PathBuf,
    _dir: Option<tempfile::TempDir>,
}

impl VoiceOutput {
    fn new(dir: tempfile::TempDir, path: PathBuf) -> Self {
        Self {
            path,
            _dir: Some(dir),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn from_test_path(path: PathBuf) -> Self {
        Self { path, _dir: None }
    }
}

pub(crate) fn render_voice(cfg: &Config, text: &str) -> Result<VoiceOutput, String> {
    render_voice_with_runtime(&TtsRuntime::from_config(cfg), cfg, text)
}

fn render_voice_with_runtime(
    runtime: &TtsRuntime,
    cfg: &Config,
    text: &str,
) -> Result<VoiceOutput, String> {
    match cfg.configured_tts() {
        Ok(Some(ConfiguredTts::ElevenLabs {
            model,
            voice,
            speed,
        })) => {
            match render_elevenlabs_voice(runtime, &cfg.state_dir, text, &model, &voice, speed) {
                Ok(output) => return Ok(output),
                Err(err) => logs::warn(format_args!(
                    "⚠️ Configured ElevenLabs TTS failed: {err}; falling back to local Voicebox."
                )),
            }
        }
        Ok(None) => {}
        Err(err) => logs::warn(format_args!(
            "⚠️ Invalid `tts` config in {}: {err}; falling back to local Voicebox.",
            cfg.gateway_config_file.display()
        )),
    }
    render_local_voicebox_with_runtime(&runtime.voicebox, &cfg.state_dir, text)
}

fn render_elevenlabs_voice(
    runtime: &TtsRuntime,
    state_dir: &Path,
    text: &str,
    model: &str,
    voice: &str,
    speed: Option<f64>,
) -> Result<VoiceOutput, String> {
    let api_key = runtime
        .elevenlabs_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{ELEVENLABS_API_KEY_ENV} is required for ElevenLabs TTS"))?;
    fs::create_dir_all(state_dir).map_err(|err| format!("create TTS state dir: {err}"))?;
    let dir = tempfile::Builder::new()
        .prefix("tts-")
        .tempdir_in(state_dir)
        .map_err(|err| format!("create TTS temp dir: {err}"))?;
    let raw_path = dir.path().join("voice-raw.opus");
    let voice_path = dir.path().join("voice.ogg");
    let url = format!(
        "{}/v1/text-to-speech/{voice}?output_format={ELEVENLABS_OUTPUT_FORMAT}",
        runtime.elevenlabs_base_url.trim_end_matches('/')
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("xi-api-key", api_key)
        .send_json(json!({
            "model_id": model,
            "text": text,
        }));
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let mut body = String::new();
            let _ = response.into_reader().take(500).read_to_string(&mut body);
            return Err(format!(
                "ElevenLabs TTS failed with status {code}: {}",
                body.trim()
            ));
        }
        Err(err) => return Err(format!("ElevenLabs TTS request failed: {err}")),
    };
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|err| format!("read ElevenLabs TTS audio: {err}"))?;
    if body.is_empty() {
        return Err("ElevenLabs TTS returned empty audio".to_string());
    }
    fs::write(&raw_path, body).map_err(|err| format!("write ElevenLabs TTS audio: {err}"))?;
    convert_voice_with_ffmpeg(&runtime.voicebox.ffmpeg, speed, &raw_path, &voice_path)?;
    Ok(VoiceOutput::new(dir, voice_path))
}

fn render_local_voicebox_with_runtime(
    runtime: &VoiceboxRuntime,
    state_dir: &Path,
    text: &str,
) -> Result<VoiceOutput, String> {
    fs::create_dir_all(state_dir).map_err(|err| format!("create TTS state dir: {err}"))?;
    let dir = tempfile::Builder::new()
        .prefix("tts-")
        .tempdir_in(state_dir)
        .map_err(|err| format!("create TTS temp dir: {err}"))?;
    let voice_path = dir.path().join("voice.ogg");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    let _server = VoiceboxServer::start_one_off(runtime)?;
    let id = start_voicebox_generation(
        &agent,
        &runtime.base_url,
        VOICEBOX_PROFILE_ID,
        VOICEBOX_ENGINE,
        text,
    )?;
    wait_for_voicebox_generation(&agent, &runtime.base_url, &id)?;
    let raw_path = runtime.generations_dir.join(format!("{id}.wav"));
    if !raw_path.exists() {
        return Err(format!(
            "Voicebox generated {id}, but {} does not exist",
            raw_path.display()
        ));
    }
    convert_voice_with_ffmpeg(&runtime.ffmpeg, runtime.speed, &raw_path, &voice_path)?;
    Ok(VoiceOutput::new(dir, voice_path))
}

struct VoiceboxServer {
    child: Option<Child>,
}

impl VoiceboxServer {
    fn start_one_off(runtime: &VoiceboxRuntime) -> Result<Self, String> {
        if voicebox_is_listening(&runtime.base_url)? {
            return Err(format!(
                "Voicebox is already running at {}; local Voicebox fallback requires a one-off gateway-launched server",
                runtime.base_url
            ));
        }
        if !runtime.executable.exists() {
            return Err(format!(
                "Voicebox is not running and executable was not found at {}",
                runtime.executable.display()
            ));
        }
        fs::create_dir_all(&runtime.model_cache_dir)
            .map_err(|err| format!("create Voicebox model cache dir: {err}"))?;
        fs::create_dir_all(&runtime.data_dir)
            .map_err(|err| format!("create Voicebox data dir: {err}"))?;
        let mut child = Command::new(&runtime.executable)
            .arg("--data-dir")
            .arg(&runtime.data_dir)
            .env("HF_HUB_CACHE", &runtime.model_cache_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("start Voicebox: {err}"))?;
        let deadline = Instant::now() + runtime.startup_timeout;
        loop {
            if voicebox_is_listening(&runtime.base_url)? {
                return Ok(Self { child: Some(child) });
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|err| format!("check Voicebox startup: {err}"))?
            {
                return Err(format!("Voicebox exited during startup with {status}"));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Voicebox did not start listening at {} within {:?}",
                    runtime.base_url, runtime.startup_timeout
                ));
            }
            thread::sleep(runtime.poll_interval);
        }
    }
}

impl Drop for VoiceboxServer {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn voicebox_is_listening(base_url: &str) -> Result<bool, String> {
    let socket = voicebox_socket(base_url)?;
    let Some(addr) = socket
        .to_socket_addrs()
        .map_err(|err| format!("resolve Voicebox address {socket}: {err}"))?
        .next()
    else {
        return Err(format!("resolve Voicebox address {socket}: no addresses"));
    };
    Ok(TcpStream::connect_timeout(&addr, VOICEBOX_PROBE_TIMEOUT).is_ok())
}

fn voicebox_socket(base_url: &str) -> Result<String, String> {
    let Some(rest) = base_url.strip_prefix("http://") else {
        return Err(format!(
            "Voicebox base URL must start with http://: {base_url}"
        ));
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || !authority.contains(':') {
        return Err(format!(
            "Voicebox base URL must include host and port: {base_url}"
        ));
    }
    Ok(authority.to_string())
}

fn start_voicebox_generation(
    agent: &ureq::Agent,
    base_url: &str,
    profile_id: &str,
    engine: &str,
    text: &str,
) -> Result<String, String> {
    let url = format!("{}/generate", base_url.trim_end_matches('/'));
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_json(json!({
            "profile_id": profile_id,
            "text": text,
            "engine": engine,
        }));
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let mut body = String::new();
            let _ = response.into_reader().take(500).read_to_string(&mut body);
            return Err(format!(
                "Voicebox TTS failed with status {code}: {}",
                body.trim()
            ));
        }
        Err(err) => return Err(format!("Voicebox TTS request failed: {err}")),
    };
    let value: serde_json::Value = response
        .into_json()
        .map_err(|err| format!("decode Voicebox generation response: {err}"))?;
    value
        .get("id")
        .or_else(|| value.get("generation_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Voicebox generation response did not include id".to_string())
}

fn wait_for_voicebox_generation(
    agent: &ureq::Agent,
    base_url: &str,
    id: &str,
) -> Result<(), String> {
    let url = format!("{}/generate/{}/status", base_url.trim_end_matches('/'), id);
    let deadline = Instant::now() + LOCAL_TTS_TIMEOUT;
    loop {
        let response = match agent.get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let mut body = String::new();
                let _ = response.into_reader().take(500).read_to_string(&mut body);
                return Err(format!(
                    "Voicebox status failed with status {code}: {}",
                    body.trim()
                ));
            }
            Err(err) => return Err(format!("Voicebox status request failed: {err}")),
        };
        let mut body = String::new();
        response
            .into_reader()
            .read_to_string(&mut body)
            .map_err(|err| format!("read Voicebox status: {err}"))?;
        match voicebox_status(&body)? {
            VoiceboxStatus::Complete => return Ok(()),
            VoiceboxStatus::Failed(detail) => {
                return Err(format!("Voicebox generation failed: {detail}"))
            }
            VoiceboxStatus::Pending => {
                if Instant::now() >= deadline {
                    return Err("Voicebox generation timed out".to_string());
                }
                thread::sleep(LOCAL_TTS_POLL_INTERVAL);
            }
        }
    }
}

enum VoiceboxStatus {
    Pending,
    Complete,
    Failed(String),
}

fn voicebox_status(body: &str) -> Result<VoiceboxStatus, String> {
    let mut saw_status = false;
    for line in body.lines() {
        let trimmed = line.trim();
        let data = if let Some(data) = trimmed.strip_prefix("data:") {
            data.trim()
        } else if trimmed.starts_with('{') {
            trimmed
        } else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|err| format!("decode Voicebox status event: {err}"))?;
        if value
            .get("completed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Ok(VoiceboxStatus::Complete);
        }
        let status = value
            .get("status")
            .or_else(|| value.get("state"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !status.is_empty() {
            saw_status = true;
        }
        if matches!(
            status.as_str(),
            "completed" | "complete" | "done" | "finished" | "success" | "succeeded"
        ) {
            return Ok(VoiceboxStatus::Complete);
        }
        if matches!(status.as_str(), "failed" | "failure" | "error") {
            return Ok(VoiceboxStatus::Failed(data.to_string()));
        }
    }
    if saw_status {
        Ok(VoiceboxStatus::Pending)
    } else {
        Err("Voicebox status response did not include status".to_string())
    }
}

fn convert_voice_with_ffmpeg(
    ffmpeg: &Path,
    speed: Option<f64>,
    raw_path: &Path,
    voice_path: &Path,
) -> Result<(), String> {
    let mut command = Command::new(ffmpeg);
    command.args(["-y", "-i"]).arg(raw_path);
    if let Some(speed) = speed {
        command.arg("-filter:a").arg(format!("atempo={speed}"));
    }
    let output = command
        .args(["-codec:a", "libopus", "-b:a", "128k", "-ar", "48000"])
        .arg(voice_path)
        .output()
        .map_err(|err| format!("start ffmpeg: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            output.status.to_string()
        } else {
            format!("{}: {stderr}", output.status)
        };
        return Err(format!("ffmpeg exited with {detail}"));
    }
    let size = fs::metadata(voice_path)
        .map_err(|err| format!("read ffmpeg voice output: {err}"))?
        .len();
    if size == 0 {
        return Err("ffmpeg produced empty voice output".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    #[test]
    fn local_voicebox_runtime_uses_gateway_data_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path(), None);

        let runtime = TtsRuntime::from_config(&cfg);

        assert_eq!(
            runtime.voicebox.executable,
            cfg.xdg_data_home
                .join("gateway/voicebox/Voicebox.app/Contents/MacOS/Voicebox")
        );
        assert_eq!(
            runtime.voicebox.model_cache_dir,
            cfg.xdg_data_home.join("gateway/voicebox/models")
        );
        assert_eq!(
            runtime.voicebox.generations_dir,
            cfg.xdg_data_home.join("gateway/voicebox/data/generations")
        );
        assert!(runtime.voicebox.executable.starts_with(&cfg.xdg_data_home));
    }

    #[test]
    fn render_voice_uses_configured_elevenlabs_before_local_voicebox() {
        let dir = tempfile::tempdir().unwrap();
        let elevenlabs = TestServer::new_many(vec![binary_response(b"eleven raw voice")]);
        let args_path = dir.path().join("ffmpeg-elevenlabs-args.txt");
        let ffmpeg = executable(
            dir.path().join("ffmpeg-elevenlabs"),
            &format!(
                r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf '%s\n' "$@" > "{}"
printf 'elevenlabs voice' > "$out"
"#,
                args_path.display()
            ),
        );
        let cfg = test_config(
            dir.path(),
            Some(json!({
                "provider": "elevenlabs",
                "model": "eleven_v3",
                "voice": "voice-abc",
                "speed": 1.25,
            })),
        );
        let runtime = TtsRuntime {
            elevenlabs_base_url: elevenlabs.base_url.clone(),
            elevenlabs_api_key: Some("secret-key".to_string()),
            voicebox: VoiceboxRuntime {
                base_url: "http://127.0.0.1:9".to_string(),
                generations_dir: dir.path().join("unused-generations"),
                ffmpeg,
                executable: dir.path().join("missing-voicebox"),
                model_cache_dir: dir.path().join("models"),
                data_dir: dir.path().join("data"),
                startup_timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(10),
                speed: Some(1.5),
            },
        };

        let output = render_voice_with_runtime(&runtime, &cfg, "say this remotely").unwrap();

        assert_eq!(
            fs::read_to_string(output.path()).unwrap(),
            "elevenlabs voice"
        );
        let request = elevenlabs.request();
        assert_eq!(
            request.path,
            "/v1/text-to-speech/voice-abc?output_format=opus_48000_128"
        );
        assert!(request.headers.contains("xi-api-key: secret-key"));
        assert!(request.body.contains(r#""model_id":"eleven_v3""#));
        assert!(request.body.contains(r#""text":"say this remotely""#));
        let args = fs::read_to_string(args_path).unwrap();
        assert!(args.contains("voice-raw.opus"));
        assert!(args.contains("-filter:a"));
        assert!(args.contains("atempo=1.25"));
        assert!(args.contains("libopus"));
    }

    #[test]
    fn render_voice_falls_back_to_local_voicebox_when_elevenlabs_fails() {
        let dir = tempfile::tempdir().unwrap();
        let elevenlabs = TestServer::new_many(vec![error_response(500, "remote failed")]);
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("local-1.wav"), "local wav").unwrap();
        let (voicebox, launcher, voicebox_base_url) = delayed_voicebox(
            dir.path(),
            vec![
                json_response(r#"{"id":"local-1"}"#),
                sse_response(r#"{"status":"completed"}"#),
            ],
        );
        let ffmpeg = executable(
            dir.path().join("ffmpeg-fallback"),
            r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf 'local fallback voice' > "$out"
"#,
        );
        let cfg = test_config(
            dir.path(),
            Some(json!({
                "provider": "elevenlabs",
                "model": "eleven_v3",
                "voice": "voice-abc",
            })),
        );
        let runtime = TtsRuntime {
            elevenlabs_base_url: elevenlabs.base_url.clone(),
            elevenlabs_api_key: Some("secret-key".to_string()),
            voicebox: VoiceboxRuntime {
                base_url: voicebox_base_url,
                generations_dir,
                ffmpeg,
                executable: launcher,
                model_cache_dir: dir.path().join("models"),
                data_dir: dir.path().join("data"),
                startup_timeout: Duration::from_secs(5),
                poll_interval: Duration::from_millis(10),
                speed: Some(1.5),
            },
        };

        let output = render_voice_with_runtime(&runtime, &cfg, "say this").unwrap();

        assert_eq!(
            fs::read_to_string(output.path()).unwrap(),
            "local fallback voice"
        );
        assert_eq!(
            elevenlabs.request().path,
            "/v1/text-to-speech/voice-abc?output_format=opus_48000_128"
        );
        assert_eq!(voicebox.request().path, "/generate");
        assert_eq!(voicebox.request().path, "/generate/local-1/status");
    }

    #[test]
    fn render_voice_falls_back_to_local_voicebox_when_tts_config_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("local-1.wav"), "local wav").unwrap();
        let (voicebox, launcher, voicebox_base_url) = delayed_voicebox(
            dir.path(),
            vec![
                json_response(r#"{"id":"local-1"}"#),
                sse_response(r#"{"status":"completed"}"#),
            ],
        );
        let ffmpeg = executable(
            dir.path().join("ffmpeg-invalid"),
            r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf 'local invalid-config voice' > "$out"
"#,
        );
        let cfg = test_config(
            dir.path(),
            Some(json!({
                "provider": "eleventlabs",
                "model": "eleven_v3",
                "voice": "voice-abc",
            })),
        );
        let runtime = TtsRuntime {
            elevenlabs_base_url: "http://127.0.0.1:9".to_string(),
            elevenlabs_api_key: Some("secret-key".to_string()),
            voicebox: VoiceboxRuntime {
                base_url: voicebox_base_url,
                generations_dir,
                ffmpeg,
                executable: launcher,
                model_cache_dir: dir.path().join("models"),
                data_dir: dir.path().join("data"),
                startup_timeout: Duration::from_secs(5),
                poll_interval: Duration::from_millis(10),
                speed: Some(1.5),
            },
        };

        let output = render_voice_with_runtime(&runtime, &cfg, "say this").unwrap();

        assert_eq!(
            fs::read_to_string(output.path()).unwrap(),
            "local invalid-config voice"
        );
        assert_eq!(voicebox.request().path, "/generate");
        assert_eq!(voicebox.request().path, "/generate/local-1/status");
    }

    #[test]
    fn render_voice_falls_back_to_local_voicebox_without_elevenlabs_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("local-1.wav"), "local wav").unwrap();
        let (voicebox, launcher, voicebox_base_url) = delayed_voicebox(
            dir.path(),
            vec![
                json_response(r#"{"id":"local-1"}"#),
                sse_response(r#"{"status":"completed"}"#),
            ],
        );
        let ffmpeg = executable(
            dir.path().join("ffmpeg-no-key"),
            r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf 'local no-key voice' > "$out"
"#,
        );
        let cfg = test_config(
            dir.path(),
            Some(json!({
                "provider": "elevenlabs",
                "model": "eleven_v3",
                "voice": "voice-abc",
            })),
        );
        let runtime = TtsRuntime {
            elevenlabs_base_url: "http://127.0.0.1:9".to_string(),
            elevenlabs_api_key: None,
            voicebox: VoiceboxRuntime {
                base_url: voicebox_base_url,
                generations_dir,
                ffmpeg,
                executable: launcher,
                model_cache_dir: dir.path().join("models"),
                data_dir: dir.path().join("data"),
                startup_timeout: Duration::from_secs(5),
                poll_interval: Duration::from_millis(10),
                speed: Some(1.5),
            },
        };

        let output = render_voice_with_runtime(&runtime, &cfg, "say this").unwrap();

        assert_eq!(
            fs::read_to_string(output.path()).unwrap(),
            "local no-key voice"
        );
        assert_eq!(voicebox.request().path, "/generate");
        assert_eq!(voicebox.request().path, "/generate/local-1/status");
    }

    #[test]
    fn render_voice_posts_to_local_voicebox_and_converts_generated_wav() {
        let dir = tempfile::tempdir().unwrap();
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("local-1.wav"), "local wav").unwrap();
        let (server, launcher, voicebox_base_url) = delayed_voicebox(
            dir.path(),
            vec![
                json_response(r#"{"id":"local-1"}"#),
                sse_response(r#"{"status":"completed"}"#),
            ],
        );
        let args_path = dir.path().join("ffmpeg-local-args.txt");
        let ffmpeg = executable(
            dir.path().join("ffmpeg-local"),
            &format!(
                r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf '%s\n' "$@" > "{}"
printf 'local sped voice' > "$out"
"#,
                args_path.display()
            ),
        );
        let runtime = VoiceboxRuntime {
            base_url: voicebox_base_url,
            generations_dir,
            ffmpeg,
            executable: launcher,
            model_cache_dir: dir.path().join("models"),
            data_dir: dir.path().join("data"),
            startup_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            speed: Some(1.5),
        };

        let output =
            render_local_voicebox_with_runtime(&runtime, dir.path(), "say this locally").unwrap();

        assert_eq!(
            fs::read_to_string(output.path()).unwrap(),
            "local sped voice"
        );
        let generate = server.request();
        assert_eq!(generate.path, "/generate");
        assert!(generate
            .body
            .contains(r#""profile_id":"60897a97-bbe7-4e3f-82f2-f27e8c377d30""#));
        assert!(generate.body.contains(r#""engine":"chatterbox_turbo""#));
        assert!(generate.body.contains(r#""text":"say this locally""#));
        let status = server.request();
        assert_eq!(status.path, "/generate/local-1/status");
        let args = fs::read_to_string(args_path).unwrap();
        assert!(args.contains("local-1.wav"));
        assert!(args.contains("-filter:a"));
        assert!(args.contains("atempo=1.5"));
        assert!(args.contains("libopus"));
    }

    #[test]
    fn render_voice_rejects_preexisting_local_voicebox_server() {
        let dir = tempfile::tempdir().unwrap();
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("local-1.wav"), "local wav").unwrap();
        let server = TestServer::new_many(vec![
            json_response(r#"{"id":"local-1"}"#),
            sse_response(r#"{"status":"completed"}"#),
        ]);
        let ffmpeg = executable(
            dir.path().join("ffmpeg-preexisting"),
            r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf 'voice' > "$out"
"#,
        );
        let runtime = VoiceboxRuntime {
            base_url: server.base_url.clone(),
            generations_dir,
            ffmpeg,
            executable: dir.path().join("missing-voicebox"),
            model_cache_dir: dir.path().join("models"),
            data_dir: dir.path().join("data"),
            startup_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(10),
            speed: None,
        };

        let err = render_local_voicebox_with_runtime(&runtime, dir.path(), "should not reuse")
            .unwrap_err();

        assert!(err.contains("already running"), "{err}");
        assert!(err.contains("one-off"), "{err}");
    }

    #[test]
    fn render_voice_launches_voicebox_only_when_needed_and_stops_it_after_generation() {
        let dir = tempfile::tempdir().unwrap();
        let port = available_port();
        let marker_path = dir.path().join("voicebox-started");
        let pid_path = dir.path().join("voicebox.pid");
        let generations_dir = dir.path().join("generations");
        fs::create_dir_all(&generations_dir).unwrap();
        fs::write(generations_dir.join("spawned-1.wav"), "local wav").unwrap();
        let _server = DelayedTestServer::new(
            port,
            marker_path.clone(),
            vec![
                json_response(r#"{"id":"spawned-1"}"#),
                sse_response(r#"{"status":"completed"}"#),
            ],
        );
        let ffmpeg = executable(
            dir.path().join("ffmpeg-spawned"),
            r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf 'voice' > "$out"
"#,
        );
        let launcher =
            voicebox_launcher(dir.path().join("voicebox"), &marker_path, Some(&pid_path));
        let runtime = VoiceboxRuntime {
            base_url: format!("http://127.0.0.1:{port}"),
            generations_dir,
            ffmpeg,
            executable: launcher,
            model_cache_dir: dir.path().join("models"),
            data_dir: dir.path().join("data"),
            startup_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            speed: None,
        };

        let output =
            render_local_voicebox_with_runtime(&runtime, dir.path(), "start server").unwrap();

        assert_eq!(fs::read_to_string(output.path()).unwrap(), "voice");
        let pid = fs::read_to_string(pid_path).unwrap();
        let still_running = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!still_running);
    }

    #[derive(Debug)]
    struct RecordedRequest {
        path: String,
        headers: String,
        body: String,
    }

    struct TestServer {
        base_url: String,
        requests: Receiver<RecordedRequest>,
        _handle: JoinHandle<()>,
    }

    impl TestServer {
        fn new_many(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let (tx, requests) = mpsc::channel();
            let handle = thread::spawn(move || {
                for response in responses {
                    loop {
                        let (stream, _) = listener.accept().unwrap();
                        let request = read_request(&stream);
                        if request.path.is_empty() {
                            continue;
                        }
                        tx.send(request).unwrap();
                        write_response(stream, &response);
                        break;
                    }
                }
            });
            Self {
                base_url,
                requests,
                _handle: handle,
            }
        }

        fn request(&self) -> RecordedRequest {
            self.requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        }
    }

    struct DelayedTestServer {
        requests: Receiver<RecordedRequest>,
        _handle: JoinHandle<()>,
    }

    impl DelayedTestServer {
        fn new(port: u16, marker_path: PathBuf, responses: Vec<String>) -> Self {
            let (tx, requests) = mpsc::channel();
            let handle = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !marker_path.exists() {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for fake Voicebox launcher"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
                for response in responses {
                    loop {
                        let (stream, _) = listener.accept().unwrap();
                        let request = read_request(&stream);
                        if request.path.is_empty() {
                            continue;
                        }
                        tx.send(request).unwrap();
                        write_response(stream, &response);
                        break;
                    }
                }
            });
            Self {
                requests,
                _handle: handle,
            }
        }

        fn request(&self) -> RecordedRequest {
            self.requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        }
    }

    fn delayed_voicebox(
        dir: &Path,
        responses: Vec<String>,
    ) -> (DelayedTestServer, PathBuf, String) {
        let port = available_port();
        let marker_path = dir.join(format!("voicebox-started-{port}"));
        let server = DelayedTestServer::new(port, marker_path.clone(), responses);
        let launcher = voicebox_launcher(dir.join(format!("voicebox-{port}")), &marker_path, None);
        (server, launcher, format!("http://127.0.0.1:{port}"))
    }

    fn available_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn voicebox_launcher(path: PathBuf, marker_path: &Path, pid_path: Option<&Path>) -> PathBuf {
        let pid_line = pid_path
            .map(|path| format!("printf '%s' \"$$\" > \"{}\"\n", path.display()))
            .unwrap_or_default();
        executable(
            path,
            &format!(
                r#"#!/bin/sh
{pid_line}printf started > "{}"
while true; do
  sleep 1
done
"#,
                marker_path.display()
            ),
        )
    }

    fn read_request(stream: &TcpStream) -> RecordedRequest {
        let mut reader = BufReader::new(stream);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let mut content_length = 0;
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            headers.push_str(line);
            headers.push('\n');
            if let Some((_, value)) = line.split_once(':') {
                if line
                    .get(.."Content-Length".len())
                    .is_some_and(|key| key.eq_ignore_ascii_case("Content-Length"))
                {
                    content_length = value.trim().parse().unwrap();
                }
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        let mut parts = first.split_whitespace();
        let _method = parts.next();
        RecordedRequest {
            path: parts.next().unwrap_or_default().to_string(),
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }

    fn write_response(mut stream: TcpStream, response: &str) {
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn sse_response(body: &str) -> String {
        let body = format!("event: status\ndata: {body}\n\n");
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn binary_response(body: &[u8]) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/opus\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        )
    }

    fn error_response(code: u16, body: &str) -> String {
        format!(
            "HTTP/1.1 {code} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn test_config(root: &Path, tts: Option<serde_json::Value>) -> Config {
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            telegram_bots: vec![crate::config::TelegramBotConfig {
                bot_token: "token".to_string(),
                chat_ids: vec![42],
                offset_file: root.join("state/gateway/telegram.offset"),
            }],
            xdg_config_home: root.join("config"),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: root.join("config/gateway/config.json"),
            codex_workdir: root.to_path_buf(),
            models: vec![crate::config::ProviderModel {
                provider: crate::provider::Provider::Codex,
                model: "gpt-test".to_string(),
                role: crate::config::ModelRole::Default,
            }],
            tts,
            state_dir: root.join("state/gateway"),
            chat_state_dir: root.join("state/gateway/chats"),
            offset_file: root.join("state/gateway/telegram.offset"),
            gateway_log_file: root.join("state/gateway/logs/gateway.log"),
            launchd_target: "gui/0/ai.gateway-test".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(5),
        }
    }

    fn executable(path: PathBuf, body: &str) -> PathBuf {
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
