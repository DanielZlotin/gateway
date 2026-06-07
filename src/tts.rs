use crate::config::Config;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const ELEVENLABS_API_KEY_ENV: &str = "ELEVENLABS_API_KEY";
const ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io";
const ELEVENLABS_VOICE_ID: &str = "cPoqAvGWCPfCfyPMwe4z";
const ELEVENLABS_MODEL_ID: &str = "eleven_v3";
const ELEVENLABS_OUTPUT_FORMAT: &str = "opus_48000_128";
const FFMPEG_BIN: &str = "ffmpeg";
const BLACK_VOICE_SPEED_FILTER: &str = "atempo=1.5";

#[derive(Debug, Clone)]
struct TtsConfig {
    base_url: String,
    api_key: String,
    voice_id: String,
    model_id: String,
    ffmpeg: PathBuf,
}

impl TtsConfig {
    fn from_env() -> Result<Self, String> {
        let api_key = std::env::var(ELEVENLABS_API_KEY_ENV)
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if api_key.is_empty() {
            return Err(format!(
                "{ELEVENLABS_API_KEY_ENV} is required for /voice mode"
            ));
        }
        Ok(Self {
            base_url: ELEVENLABS_BASE_URL.to_string(),
            api_key,
            voice_id: ELEVENLABS_VOICE_ID.to_string(),
            model_id: ELEVENLABS_MODEL_ID.to_string(),
            ffmpeg: PathBuf::from(FFMPEG_BIN),
        })
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
    let tts = TtsConfig::from_env()?;
    render_voice_with_config(&tts, &cfg.state_dir, text)
}

fn render_voice_with_config(
    tts: &TtsConfig,
    state_dir: &Path,
    text: &str,
) -> Result<VoiceOutput, String> {
    fs::create_dir_all(state_dir).map_err(|err| format!("create TTS state dir: {err}"))?;
    let dir = tempfile::Builder::new()
        .prefix("tts-")
        .tempdir_in(state_dir)
        .map_err(|err| format!("create TTS temp dir: {err}"))?;
    let raw_path = dir.path().join("voice-raw.ogg");
    let voice_path = dir.path().join("voice.ogg");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    let bytes = synthesize_elevenlabs_bytes(
        &agent,
        &tts.base_url,
        &tts.api_key,
        &tts.voice_id,
        &tts.model_id,
        text,
    )?;
    fs::write(&raw_path, bytes).map_err(|err| format!("write ElevenLabs audio: {err}"))?;
    speed_up_voice_with_ffmpeg(&tts.ffmpeg, &raw_path, &voice_path)?;
    Ok(VoiceOutput::new(dir, voice_path))
}

fn synthesize_elevenlabs_bytes(
    agent: &ureq::Agent,
    base_url: &str,
    api_key: &str,
    voice_id: &str,
    model_id: &str,
    text: &str,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "{}/v1/text-to-speech/{}?output_format={}",
        base_url.trim_end_matches('/'),
        voice_id,
        ELEVENLABS_OUTPUT_FORMAT
    );
    let response = agent
        .post(&url)
        .set("xi-api-key", api_key)
        .set("Content-Type", "application/json")
        .send_json(json!({
            "text": text,
            "model_id": model_id,
        }));
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let mut body = String::new();
            let _ = response.into_reader().take(500).read_to_string(&mut body);
            let detail = body.trim();
            if detail.is_empty() {
                return Err(format!("ElevenLabs TTS failed with status {code}"));
            }
            return Err(format!(
                "ElevenLabs TTS failed with status {code}: {detail}"
            ));
        }
        Err(err) => return Err(format!("ElevenLabs TTS request failed: {err}")),
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| format!("read ElevenLabs audio: {err}"))?;
    if bytes.is_empty() {
        return Err("ElevenLabs returned empty audio".to_string());
    }
    Ok(bytes)
}

fn speed_up_voice_with_ffmpeg(
    ffmpeg: &Path,
    raw_path: &Path,
    voice_path: &Path,
) -> Result<(), String> {
    let output = Command::new(ffmpeg)
        .args(["-y", "-i"])
        .arg(raw_path)
        .args([
            "-filter:a",
            BLACK_VOICE_SPEED_FILTER,
            "-codec:a",
            "libopus",
            "-b:a",
            "128k",
            "-ar",
            "48000",
        ])
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
    fn render_voice_posts_to_elevenlabs_and_runs_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::new(binary_response("raw voice"));
        let args_path = dir.path().join("ffmpeg-args.txt");
        let ffmpeg = executable(
            dir.path().join("ffmpeg"),
            &format!(
                r#"#!/bin/sh
out=""
for arg in "$@"; do
  out="$arg"
done
printf '%s\n' "$@" > "{}"
printf 'sped voice' > "$out"
"#,
                args_path.display()
            ),
        );
        let tts = TtsConfig {
            base_url: server.base_url.clone(),
            api_key: "secret-key".to_string(),
            voice_id: "voice-1".to_string(),
            model_id: "eleven_v3".to_string(),
            ffmpeg,
        };

        let output = render_voice_with_config(&tts, dir.path(), "say this").unwrap();

        assert_eq!(fs::read_to_string(output.path()).unwrap(), "sped voice");
        let request = server.request();
        assert_eq!(
            request.path,
            "/v1/text-to-speech/voice-1?output_format=opus_48000_128"
        );
        assert!(request.headers.contains("xi-api-key: secret-key"));
        assert!(request.body.contains(r#""text":"say this""#));
        assert!(request.body.contains(r#""model_id":"eleven_v3""#));
        let args = fs::read_to_string(args_path).unwrap();
        assert!(args.contains("atempo=1.5"));
        assert!(args.contains("libopus"));
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
        fn new(response: String) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let (tx, requests) = mpsc::channel();
            let handle = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let request = read_request(&stream);
                tx.send(request).unwrap();
                write_response(stream, &response);
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

    fn binary_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/ogg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn executable(path: PathBuf, body: &str) -> PathBuf {
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
