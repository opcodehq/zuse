//! Short, authenticated dictation; no chat, realtime session or stored audio.
use super::*;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

const ENDPOINT: &str = "https://chatgpt.com/backend-api/transcribe";
const MAX_BYTES: usize = 10 * 1024 * 1024;

fn recording(params: &Value) -> Result<Vec<u8>, RpcError> {
    let encoded = params["audio"]
        .as_str()
        .filter(|value| value.len() <= MAX_BYTES.div_ceil(3) * 4)
        .ok_or_else(|| RpcError::BadParams("Recording is missing or too large.".into()))?;
    let audio = STANDARD
        .decode(encoded)
        .map_err(|_| RpcError::BadParams("Invalid recording.".into()))?;
    // Accept only our bounded mono PCM16 WAV capture, up to 150 seconds.
    let valid = audio.len() >= 44
        && audio.len() <= 16_000 * 2 * 150 + 44
        && &audio[..4] == b"RIFF"
        && &audio[8..16] == b"WAVEfmt "
        && audio[16..36]
            == [
                16, 0, 0, 0, 1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0,
            ]
        && &audio[36..40] == b"data"
        && u32::from_le_bytes(audio[40..44].try_into().unwrap()) as usize == audio.len() - 44
        && u32::from_le_bytes(audio[4..8].try_into().unwrap()) as usize == audio.len() - 8
        && (audio.len() - 44) % 2 == 0;
    if !valid {
        return Err(RpcError::BadParams("Invalid dictation recording.".into()));
    }
    Ok(audio)
}

fn upload(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    account: Option<&str>,
    audio: Vec<u8>,
) -> reqwest::RequestBuilder {
    let boundary = format!("zuse-{}", uuid::Uuid::new_v4());
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"recording.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    body.extend(audio);
    body.extend(format!("\r\n--{boundary}--\r\n").bytes());
    let request = client
        .post(endpoint)
        .bearer_auth(token)
        .header("User-Agent", "Zuse-Mobile-Voice/1")
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body);
    if let Some(account) = account {
        request.header("ChatGPT-Account-Id", account)
    } else {
        request
    }
}

fn transcript(status: reqwest::StatusCode, body: &[u8]) -> Result<String, RpcError> {
    if !status.is_success() {
        let message = match status.as_u16() {
            401 => "Codex sign-in expired. Sign in again in Settings → Agents.",
            403 => "ChatGPT blocked transcription. Your draft is safe; try again shortly.",
            429 => "Dictation is temporarily rate limited. Try again shortly.",
            _ => "ChatGPT could not transcribe this recording. Your draft is safe.",
        };
        // Provider bodies can contain credentials, HTML or audio; never forward them.
        return Err(RpcError::Failed(message.into()));
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| RpcError::Failed("Transcription returned an invalid response.".into()))?;
    value["text"]
        .as_str()
        .or_else(|| value["transcript"].as_str())
        .map(|text| text.trim().to_owned())
        .ok_or_else(|| RpcError::Failed("Transcription returned no text.".into()))
}

impl EngineRpc {
    pub(super) async fn transcribe_dictation(&self, params: Value) -> Result<RpcReply, RpcError> {
        let audio = recording(&params)?;
        let (token, account) = self
            .agent_accounts
            .dictation_auth()
            .await
            .map_err(|error| RpcError::Failed(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RpcError::Failed("Transcription is unavailable.".into()))?;
        let response = upload(&client, ENDPOINT, &token, account.as_deref(), audio)
            .send()
            .await
            .map_err(|_| {
                RpcError::Failed("Transcription could not connect. Your draft is safe.".into())
            })?;
        let status = response.status();
        // Responses should only be a short transcript, not an unbounded download.
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RpcError::Failed("Transcription was interrupted.".into()))?
        {
            if body.len() + chunk.len() > 1024 * 1024 {
                return Err(RpcError::Failed(
                    "Transcription response is too large.".into(),
                ));
            }
            body.extend(chunk);
        }
        RpcReply::value(&json!({"text":transcript(status, &body)?}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_electron_upload_contract_without_exposing_credentials() {
        let request = upload(
            &reqwest::Client::new(),
            ENDPOINT,
            "fixture-token",
            Some("fixture-account"),
            b"fixture-audio".to_vec(),
        )
        .build()
        .unwrap();
        assert_eq!(request.url().as_str(), ENDPOINT);
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.headers()["authorization"], "Bearer fixture-token");
        assert_eq!(request.headers()["chatgpt-account-id"], "fixture-account");
        let body = request.body().unwrap().as_bytes().unwrap();
        assert!(
            String::from_utf8_lossy(body).contains("name=\"file\"; filename=\"recording.wav\"")
        );
        assert!(String::from_utf8_lossy(body).contains("Content-Type: audio/wav"));
        assert!(String::from_utf8_lossy(body).contains("fixture-audio"));
        let without_account = upload(
            &reqwest::Client::new(),
            ENDPOINT,
            "fixture-token",
            None,
            vec![],
        )
        .build()
        .unwrap();
        assert!(!without_account.headers().contains_key("chatgpt-account-id"));
    }
    #[test]
    fn accepts_electron_response_formats_and_silence_but_redacts_errors() {
        assert_eq!(
            transcript(reqwest::StatusCode::OK, br#"{"text":" hello "}"#).unwrap(),
            "hello"
        );
        assert_eq!(
            transcript(reqwest::StatusCode::OK, br#"{"transcript":""}"#).unwrap(),
            ""
        );
        assert!(transcript(reqwest::StatusCode::OK, b"{}").is_err());
        let error = transcript(reqwest::StatusCode::UNAUTHORIZED, b"fixture-secret")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("fixture-secret"));
        assert!(error.contains("sign-in"));
    }
    #[test]
    fn validates_pcm_wav_before_auth_or_upload() {
        let mut audio = b"RIFF".to_vec();
        audio.extend(38u32.to_le_bytes());
        audio.extend(b"WAVEfmt ");
        audio.extend([
            16, 0, 0, 0, 1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0,
        ]);
        audio.extend(b"data");
        audio.extend(2u32.to_le_bytes());
        audio.extend([0, 0]);
        assert_eq!(
            recording(&json!({"audio":STANDARD.encode(&audio)})).unwrap(),
            audio
        );
        audio[40] = 3;
        assert!(recording(&json!({"audio":STANDARD.encode(&audio)})).is_err());
    }

    #[test]
    fn rejects_malformed_and_oversized_recordings_before_auth() {
        for audio in ["", "!!!!", "UkxGRg=="] {
            assert!(recording(&json!({"audio":audio})).is_err());
        }
        assert!(recording(&json!({"audio":"a".repeat(MAX_BYTES.div_ceil(3)*4+1)})).is_err());
    }
}
