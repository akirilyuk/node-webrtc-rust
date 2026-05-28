#[cfg(feature = "live")]
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

#[cfg(feature = "live")]
pub(crate) async fn google_access_token() -> SpeechResult<Option<String>> {
    if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(Some(token.trim().to_string()));
        }
    }

    let credentials_path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").map_err(|_| {
        SpeechError::Config(
            "missing Google credentials: set GOOGLE_APPLICATION_CREDENTIALS, GOOGLE_API_KEY, or GOOGLE_ACCESS_TOKEN"
                .into(),
        )
    })?;

    let key_json = tokio::fs::read_to_string(&credentials_path)
        .await
        .map_err(|err| SpeechError::Config(format!("read GOOGLE_APPLICATION_CREDENTIALS: {err}")))?;
    let key: yup_oauth2::ServiceAccountKey = serde_json::from_str(&key_json).map_err(|err| {
        SpeechError::Config(format!("parse service account JSON: {err}"))
    })?;
    let auth = yup_oauth2::ServiceAccountAuthenticator::builder(key)
        .build()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    let token = auth
        .token(&["https://www.googleapis.com/auth/cloud-platform"])
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    Ok(token.token().map(|t| t.to_string()))
}

#[cfg(feature = "live")]
pub(crate) fn google_api_key() -> Option<String> {
    std::env::var("GOOGLE_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
}
